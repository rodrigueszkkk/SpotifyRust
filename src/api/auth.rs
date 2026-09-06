use anyhow::{anyhow, Result};
use directories::ProjectDirs;
use log::{info, warn};
use rspotify::{prelude::*, scopes, AuthCodePkceSpotify, Credentials, OAuth};
use std::fs;
use std::path::PathBuf;
use tiny_http::{Response, Server};

/// Recupera o diretório de dados do projeto de forma cross-platform
pub fn get_data_dir() -> Option<PathBuf> {
    if let Some(proj_dirs) = ProjectDirs::from("com", "spotify-rust", "spotify-rust") {
        let dir = proj_dirs.data_dir().to_path_buf();
        if !dir.exists() {
            let _ = fs::create_dir_all(&dir);
        }
        Some(dir)
    } else {
        warn!("Não foi possível determinar o diretório de dados do projeto.");
        None
    }
}

/// Autentica via OAuth PKCE com a Web API
pub async fn authenticate_web_api(
    on_auth_url: impl FnOnce(String) + Send + 'static,
) -> Result<AuthCodePkceSpotify> {
    info!("Iniciando autenticação Web API (OAuth PKCE)...");

    // Obter credenciais das variáveis de ambiente
    let client_id = std::env::var("SPOTIFY_CLIENT_ID").unwrap_or_else(|_| "".to_string());

    let is_mock = client_id.is_empty();
    let auth_client_id = if is_mock {
        info!("SPOTIFY_CLIENT_ID não configurado. Executando fluxo de Autenticação MOCK...");
        "mock_client_id".to_string()
    } else {
        client_id
    };

    // Configuração do rspotify
    let creds = Credentials::new_pkce(&auth_client_id);
    let oauth = OAuth {
        redirect_uri: "http://127.0.0.1:8888/callback".to_string(),
        scopes: scopes!(
            "user-read-private user-read-email streaming user-read-playback-state user-modify-playback-state user-library-read playlist-read-private playlist-read-collaborative user-read-recently-played"
        ),
        ..Default::default()
    };

    let mut spotify = AuthCodePkceSpotify::new(creds, oauth);

    let cache_path = get_data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("token.json");
    spotify.config.token_cached = true;
    spotify.config.cache_path = cache_path.clone();

    // Tentar ler token do cache primeiro (só se ainda for válido)
    if let Ok(Some(token)) = spotify.read_token_cache(false).await {
        info!("Token válido encontrado no cache local! Restaurando sessão...");
        if let Ok(mut token_lock) = spotify.get_token().lock().await {
            *token_lock = Some(token);
        }
        return Ok(spotify);
    }

    info!("Iniciando fluxo de login no navegador...");
    let auth_url = if is_mock {
        "http://127.0.0.1:8888/callback?code=simulacao_oauth".to_string()
    } else {
        spotify.get_authorize_url(None)?
    };

    println!("\n╔══════════════════════════════════════════════════════════════════════════╗");
    println!("║ AUTORIZAÇÃO SPOTIFY NECESSÁRIA:                                          ║");
    println!("║ Link para login no navegador:                                            ║");
    println!("║ {}                                                                       ║", auth_url);
    println!("╚══════════════════════════════════════════════════════════════════════════╝\n");

    on_auth_url(auth_url.clone());

    if webbrowser::open(&auth_url).is_err() {
        warn!(
            "Não foi possível abrir o navegador automaticamente. Acesse: {}",
            auth_url
        );
    }

    info!("Aguardando callback na porta 8888...");

    // Roda o servidor bloqueante em uma thread separada para não travar o Tokio
    let code = tokio::task::spawn_blocking(move || {
        let server = Server::http("127.0.0.1:8888")
            .map_err(|e| anyhow!("Falha ao criar servidor local: {}", e))?;

        for request in server.incoming_requests() {
            let url = request.url().to_string();
            if url.starts_with("/callback") {
                info!("Callback recebido!");

                let html = "<html><body style='font-family:sans-serif; background:#1e1e1e; color:#fff; display:flex; flex-direction:column; align-items:center; justify-content:center; height:100vh;'><h1 style='color:#FA243C'>Login concluído com sucesso!</h1><p>Você pode fechar esta aba e voltar ao aplicativo Spotify Rust.</p><script>window.close();</script></body></html>";
                let response = Response::from_string(html).with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
                );
                let _ = request.respond(response);

                // Extrair código
                let parts: Vec<&str> = url.split("code=").collect();
                if parts.len() > 1 {
                    return Ok(parts[1].split('&').next().unwrap_or("").to_string());
                }
            } else {
                let _ = request.respond(Response::from_string("Not Found").with_status_code(404));
            }
        }
        Err(anyhow!("Servidor encerrou sem receber o callback"))
    })
    .await??;

    info!("Obtendo token via código de autorização...");
    if is_mock {
        info!("(Modo MOCK): Token simulado.");
    } else {
        spotify.request_token(&code).await?;
        spotify.write_token_cache().await?;
    }

    info!("Autenticação Web API completa!");
    Ok(spotify)
}
