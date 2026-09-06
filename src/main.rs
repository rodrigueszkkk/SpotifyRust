// Cache invalidation trigger

slint::include_modules!();

mod api;
mod audio;
mod cache;
mod models;
mod ui;
mod utils;

use anyhow::Result;
use log::{error, info, warn};
use rspotify::clients::BaseClient;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

thread_local! {
    static RECENT_TRACKS_MODEL: RefCell<Option<Rc<VecModel<TrackItem>>>> = RefCell::new(None);
    static PLAYLISTS_MODEL: RefCell<Option<Rc<VecModel<PlaylistItem>>>> = RefCell::new(None);
    static QUEUE_TRACKS_MODEL: RefCell<Option<Rc<VecModel<TrackItem>>>> = RefCell::new(None);
}

async fn load_image_cover_buffer(url: &str) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    if url.is_empty() {
        return None;
    }
    let data_dir = api::auth::get_data_dir()?;
    let covers_dir = data_dir.join("covers");
    let _ = std::fs::create_dir_all(&covers_dir);
    let file_name = format!("{:x}.jpg", md5::compute(url));
    let file_path = covers_dir.join(file_name);

    let bytes_opt = if file_path.exists() {
        std::fs::read(&file_path).ok()
    } else {
        match reqwest::get(url).await {
            Ok(resp) => {
                if let Ok(bytes) = resp.bytes().await {
                    let _ = std::fs::write(&file_path, &bytes);
                    Some(bytes.to_vec())
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    };

    if let Some(bytes) = bytes_opt {
        tokio::task::spawn_blocking(move || {
            if let Ok(dynamic_img) = image::load_from_memory(&bytes) {
                let rgba = dynamic_img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
                buf.make_mut_bytes().copy_from_slice(rgba.as_raw());
                Some(buf)
            } else {
                None
            }
        }).await.unwrap_or(None)
    } else {
        None
    }
}

struct PlaybackQueue {
    tracks: Vec<models::Track>,
    current_idx: usize,
    lyrics: Vec<models::LyricLine>,
    active_lyric_idx: Option<usize>,
    last_played_uri: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Inicializa o logger
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info"));
    info!("Iniciando Spotify Rust (Estilo Apple Music)...");

    // Carrega variáveis de ambiente do .env
    dotenv::dotenv().ok();

    // Inicializa o banco de dados SQLite local
    let _db = cache::init_db()?;

    // Cria a janela do Slint na UI thread
    let app = AppWindow::new()?;
    let app_weak = app.as_weak();

    // Carrega preferências salvas da sessão anterior
    let saved_config = utils::config::AppConfig::load();
    app.set_volume(saved_config.volume);
    if !saved_config.last_view.is_empty() && saved_config.last_view != "lyrics" {
        app.set_active_nav(saved_config.last_view.clone().into());
        app.set_current_view(saved_config.last_view.clone().into());
    }

    // Canais de comando e eventos para o player de áudio (capacidade 32)
    let (audio_cmd_tx, mut audio_cmd_rx) = mpsc::channel::<audio::player::PlayerCommand>(32);
    let (audio_event_tx, mut audio_event_rx) = mpsc::channel::<audio::player::PlayerEventMsg>(32);

    // Estado compartilhado da fila de reprodução (Thread-safe)
    let queue_state = Arc::new(Mutex::new(PlaybackQueue {
        tracks: Vec::new(),
        current_idx: 0,
        lyrics: Vec::new(),
        active_lyric_idx: None,
        last_played_uri: None,
    }));

    // URL de autenticação atual para o botão da interface
    let current_auth_url = Arc::new(Mutex::new(String::new()));
    let current_auth_url_clone = current_auth_url.clone();

    // Token de acesso para chamadas diretas REST
    let access_token_shared = Arc::new(Mutex::new(String::new()));
    let access_token_shared_clone = access_token_shared.clone();

    // Canal interno para passar o player iniciado para o command loop
    let (player_ready_tx, mut player_ready_rx) = mpsc::channel::<audio::player::AudioPlayer>(1);

    // Tarefa dedicada para o command loop do áudio
    let audio_event_tx_loop = audio_event_tx.clone();
    tokio::spawn(async move {
        info!("Aguardando inicialização do player de áudio...");
        if let Some(player) = player_ready_rx.recv().await {
            info!("Player recebido! Iniciando loop de controle.");
            player.run_command_loop(audio_cmd_rx, audio_event_tx_loop).await;
        } else {
            while let Some(_) = audio_cmd_rx.recv().await {}
        }
    });

    let username = std::env::var("SPOTIFY_USER")
        .unwrap_or_default()
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();
    let password = std::env::var("SPOTIFY_PASS")
        .unwrap_or_default()
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();

    // Inicia autenticação Web API em background
    let app_weak_auth = app_weak.clone();
    let player_ready_tx_clone = player_ready_tx.clone();
    let username_clone = username.clone();
    let password_clone = password.clone();

    tokio::spawn(async move {
        let app_w_url = app_weak_auth.clone();
        let auth_url_store = current_auth_url_clone.clone();

        let auth_result = api::auth::authenticate_web_api(move |url| {
            *auth_url_store.lock().unwrap() = url;
            let app_w = app_w_url.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = app_w.upgrade() {
                    app.set_is_authenticating(true);
                    app.set_auth_status_text("Complete a autorização no navegador aberto para carregar suas músicas".into());
                }
            });
        }).await;

        match auth_result {
            Ok(spotify) => {
                info!("Autenticação Web API completada com sucesso!");

                // Oculta o banner de autenticação
                let app_w = app_weak_auth.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = app_w.upgrade() {
                        app.set_is_authenticating(false);
                    }
                });

                // Extrai o access_token para a API REST e librespot
                let mut access_token = String::new();
                if let Ok(token_lock) = spotify.get_token().lock().await {
                    if let Some(ref token) = *token_lock {
                        access_token = token.access_token.clone();
                    }
                }

                if !access_token.is_empty() {
                    *access_token_shared_clone.lock().unwrap() = access_token.clone();

                    // 1. CARREGA PLAYLISTS DO USUÁRIO IMEDIATAMENTE VIA REST
                    let token_pl = access_token.clone();
                    let app_w_pl = app_weak_auth.clone();
                    tokio::spawn(async move {
                        info!("Iniciando carregamento de playlists...");
                        match api::client::fetch_user_playlists(&token_pl).await {
                            Ok(playlists) => {
                                info!("Encontradas {} playlists reais!", playlists.len());
                                let playlists_for_ui = playlists.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    let items: Vec<PlaylistItem> = playlists_for_ui
                                        .iter()
                                        .map(|p| PlaylistItem {
                                            id: p.id.clone().into(),
                                            name: p.name.clone().into(),
                                            image_url: p.image_url.clone().unwrap_or_default().into(),
                                            track_count: p.tracks_total as i32,
                                            cover: slint::Image::default(),
                                            has_cover: false,
                                        })
                                        .collect();

                                    let model = Rc::new(VecModel::from(items));
                                    PLAYLISTS_MODEL.with(|m| {
                                        *m.borrow_mut() = Some(model.clone());
                                    });
                                    if let Some(app) = app_w_pl.upgrade() {
                                        app.set_playlists(ModelRc::from(model));
                                    }
                                });

                                // Baixa e renderiza capas das playlists em background
                                tokio::spawn(async move {
                                    for (idx, p) in playlists.into_iter().enumerate() {
                                        if let Some(url) = p.image_url {
                                            if !url.is_empty() {
                                                if let Some(buf) = load_image_cover_buffer(&url).await {
                                                    let _ = slint::invoke_from_event_loop(move || {
                                                        PLAYLISTS_MODEL.with(|m| {
                                                            if let Some(ref model) = *m.borrow() {
                                                                if let Some(mut item) = model.row_data(idx) {
                                                                    item.cover = slint::Image::from_rgba8(buf);
                                                                    item.has_cover = true;
                                                                    model.set_row_data(idx, item);
                                                                }
                                                            }
                                                        });
                                                    });
                                                }
                                            }
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                error!("Erro ao buscar playlists do usuário: {}", e);
                            }
                        }
                    });

                    // 2. CARREGA FAIXAS RECENTES (OUVIR AGORA) IMEDIATAMENTE VIA REST
                    let token_rec = access_token.clone();
                    let app_w_rec = app_weak_auth.clone();
                    tokio::spawn(async move {
                        info!("Iniciando carregamento de faixas tocadas recentemente...");
                        match api::client::fetch_recently_played(&token_rec, 20).await {
                            Ok(recents) => {
                                info!("Encontradas {} faixas tocadas recentemente!", recents.len());
                                // Deduplica histórico para evitar cards e linhas repetidas
                                let mut unique_recents: Vec<models::Track> = Vec::new();
                                for t in recents {
                                    if !unique_recents.iter().any(|existing| existing.id == t.id || existing.uri == t.uri) {
                                        unique_recents.push(t);
                                    }
                                }
                                let recents = unique_recents;
                                let recents_for_ui = recents.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    let items: Vec<TrackItem> = recents_for_ui
                                        .iter()
                                        .map(|t| {
                                            let duration = t.formatted_duration();
                                            TrackItem {
                                                id: t.id.clone().into(),
                                                uri: t.uri.clone().into(),
                                                title: t.title.clone().into(),
                                                artist: t.artist.clone().into(),
                                                album: t.album.clone().into(),
                                                duration: duration.into(),
                                                duration_ms: t.duration_ms as i32,
                                                cover_url: t.cover_url.clone().unwrap_or_default().into(),
                                                cover: slint::Image::default(),
                                                has_cover: false,
                                            }
                                        })
                                        .collect();

                                    let model = Rc::new(VecModel::from(items));
                                    RECENT_TRACKS_MODEL.with(|m| {
                                        *m.borrow_mut() = Some(model.clone());
                                    });
                                    if let Some(app) = app_w_rec.upgrade() {
                                        app.set_recent_tracks(ModelRc::from(model));
                                    }
                                });

                                // Baixa e renderiza capas das faixas recentes em background
                                tokio::spawn(async move {
                                    for (idx, t) in recents.into_iter().enumerate() {
                                        if let Some(url) = t.cover_url {
                                            if !url.is_empty() {
                                                if let Some(buf) = load_image_cover_buffer(&url).await {
                                                    let _ = slint::invoke_from_event_loop(move || {
                                                        RECENT_TRACKS_MODEL.with(|m| {
                                                            if let Some(ref model) = *m.borrow() {
                                                                if let Some(mut item) = model.row_data(idx) {
                                                                    item.cover = slint::Image::from_rgba8(buf);
                                                                    item.has_cover = true;
                                                                    model.set_row_data(idx, item);
                                                                }
                                                            }
                                                        });
                                                    });
                                                }
                                            }
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                warn!("Nenhuma faixa recente encontrada: {}", e);
                            }
                        }
                    });

                    // 3. INICIALIZA AUDIO PLAYER EM PARALELO
                    let token_audio = access_token.clone();
                    let p_tx = player_ready_tx_clone.clone();
                    let user = username_clone.clone();
                    let pass = password_clone.clone();

                    tokio::spawn(async move {
                        info!("Conectando áudio librespot via Token OAuth...");
                        match audio::player::AudioPlayer::new_with_token(user.clone(), token_audio).await {
                            Ok(player) => {
                                info!("Librespot conectado com sucesso usando token OAuth!");
                                let _ = p_tx.send(player).await;
                            }
                            Err(e) => {
                                warn!("Falha ao conectar com token: {}. Tentando senha...", e);
                                if !user.is_empty() && !pass.is_empty() {
                                    match audio::player::AudioPlayer::new_with_password(user, pass).await {
                                        Ok(player) => {
                                            info!("Librespot conectado com sucesso usando senha!");
                                            let _ = p_tx.send(player).await;
                                        }
                                        Err(e2) => {
                                            error!("Falha ao conectar librespot com senha: {}", e2);
                                        }
                                    }
                                }
                            }
                        }
                    });
                }
            }
            Err(e) => {
                error!("Falha na autenticação Spotify Web API: {}", e);
                let app_w = app_weak_auth.clone();
                let err_msg = format!("Erro na autenticação: {}", e);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(app) = app_w.upgrade() {
                        app.set_is_authenticating(true);
                        app.set_auth_status_text(err_msg.into());
                    }
                });
            }
        }
    });

    // Monitoramento em tempo real dos eventos do player (Posição & Estado)
    let app_weak_events = app_weak.clone();
    let queue_state_events = queue_state.clone();
    let _audio_cmd_tx_events = audio_cmd_tx.clone();

    tokio::spawn(async move {
        while let Some(event) = audio_event_rx.recv().await {
            match event {
                audio::player::PlayerEventMsg::StateChanged { is_playing } => {
                    let app_w = app_weak_events.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_w.upgrade() {
                            app.set_is_playing(is_playing);
                        }
                    });
                }
                audio::player::PlayerEventMsg::Position(ms) => {
                    let app_w = app_weak_events.clone();
                    let q_state = queue_state_events.clone();

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_w.upgrade() {
                            let mut state = q_state.lock().unwrap();
                            let total_duration = if let Some(track) = state.tracks.get(state.current_idx) {
                                track.duration_ms
                            } else {
                                1
                            };

                            let progress = (ms as f32 / total_duration as f32).clamp(0.0, 1.0);
                            app.set_playback_progress(progress);

                            // Formatação de tempo
                            let elapsed_sec = ms / 1000;
                            app.set_time_elapsed(format!("{}:{:02}", elapsed_sec / 60, elapsed_sec % 60).into());

                            let remaining_ms = total_duration.saturating_sub(ms);
                            let rem_sec = remaining_ms / 1000;
                            app.set_time_remaining(format!("-{}:{:02}", rem_sec / 60, rem_sec % 60).into());

                            // Sincronização de Letras (Identifica a linha ativa)
                            if !state.lyrics.is_empty() {
                                let mut active_idx = 0;
                                for (i, line) in state.lyrics.iter().enumerate() {
                                    if ms >= line.timestamp_ms {
                                        active_idx = i;
                                    } else {
                                        break;
                                    }
                                }

                                if state.active_lyric_idx != Some(active_idx) {
                                    state.active_lyric_idx = Some(active_idx);
                                    app.set_active_lyric_idx(active_idx as i32);
                                }
                            }
                        }
                    });
                }
                audio::player::PlayerEventMsg::EndOfTrack => {
                    let app_w = app_weak_events.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = app_w.upgrade() {
                            println!("[AudioPlayer] Fim da faixa alcançado, avançando automaticamente para próxima faixa...");
                            app.invoke_next_track();
                        }
                    });
                }
            }
        }
    });

    // Helper para tocar uma faixa e buscar suas letras sincronizadas
    let play_track_fn = {
        let app_weak = app_weak.clone();
        let queue_state = queue_state.clone();
        let audio_cmd_tx = audio_cmd_tx.clone();

        Rc::new(move |track: models::Track| {
            // 1. Deduplicação: ignora se a mesma faixa já está sendo processada
            {
                let mut state = queue_state.lock().unwrap();
                if Some(&track.uri) == state.last_played_uri.as_ref() {
                    println!("[PlayTrack] Mesma faixa já processada ({}), ignorando para não duplicar histórico nem cards.", track.title);
                    return;
                }
                state.last_played_uri = Some(track.uri.clone());
            }

            info!("Tocando faixa: '{}' - '{}'", track.title, track.artist);
            println!("[PlayTrack] Solicitado tocar: '{}' - '{}' | cover_url: {:?}", track.title, track.artist, track.cover_url);

            if let Some(app) = app_weak.upgrade() {
                app.set_current_track_title(track.title.clone().into());
                app.set_current_track_artist(track.artist.clone().into());
                app.set_is_playing(true);
                app.set_playback_progress(0.0);
                app.set_time_elapsed("0:00".into());
                app.set_time_remaining(format!("-{}", track.formatted_duration()).into());
            }

            // 2. Consome a faixa da fila "A Seguir" no Slint: remove a que começou a tocar
            let curr_uri_q = track.uri.clone();
            let curr_id_q = track.id.clone();
            let _ = slint::invoke_from_event_loop(move || {
                QUEUE_TRACKS_MODEL.with(|m| {
                    if let Some(ref model) = *m.borrow() {
                        for i in 0..model.row_count() {
                            if let Some(row) = model.row_data(i) {
                                if row.uri == curr_uri_q || row.id == curr_id_q {
                                    model.remove(i);
                                    println!("[Queue] Faixa consumida removida da fila 'A Seguir': {}", row.title);
                                    break;
                                }
                            }
                        }
                    }
                });
            });

            // 3. Atualiza "Tocadas Recentemente" inserindo no topo e deduplicando
            let track_rec = track.clone();
            let _ = slint::invoke_from_event_loop(move || {
                RECENT_TRACKS_MODEL.with(|m| {
                    if let Some(ref model) = *m.borrow() {
                        for i in 0..model.row_count() {
                            if let Some(row) = model.row_data(i) {
                                if row.uri == track_rec.uri || row.id == track_rec.id {
                                    model.remove(i);
                                    break;
                                }
                            }
                        }
                        let duration = track_rec.formatted_duration();
                        let new_item = TrackItem {
                            id: track_rec.id.clone().into(),
                            uri: track_rec.uri.clone().into(),
                            title: track_rec.title.clone().into(),
                            artist: track_rec.artist.clone().into(),
                            album: track_rec.album.clone().into(),
                            duration: duration.into(),
                            duration_ms: track_rec.duration_ms as i32,
                            cover_url: track_rec.cover_url.clone().unwrap_or_default().into(),
                            cover: slint::Image::default(),
                            has_cover: false,
                        };
                        model.insert(0, new_item);
                    }
                });
            });

            // Envia comando de load não-bloqueante
            let _ = audio_cmd_tx.try_send(audio::player::PlayerCommand::Load {
                uri: track.uri.clone(),
                play: true,
            });

            // Dispara carregamento de capa de álbum e geração de Ambient Art desfocada
            if let Some(cover_url) = track.cover_url.clone() {
                println!("[Cover Pipeline] Iniciando pipeline de capa para URL: {}", cover_url);
                let app_w_cov = app_weak.clone();
                tokio::spawn(async move {
                    if let Some(data_dir) = api::auth::get_data_dir() {
                        let covers_dir = data_dir.join("covers");
                        let _ = std::fs::create_dir_all(&covers_dir);
                        let file_name = format!("{:x}.jpg", md5::compute(&cover_url));
                        let file_path = covers_dir.join(file_name);

                        let bytes_opt = if file_path.exists() {
                            println!("[Cover Pipeline] Capa encontrada no cache local: {:?}", file_path);
                            std::fs::read(&file_path).ok()
                        } else {
                            println!("[Cover Pipeline] Baixando capa da rede: {}", cover_url);
                            match reqwest::get(&cover_url).await {
                                Ok(resp) => {
                                    if let Ok(bytes) = resp.bytes().await {
                                        let _ = std::fs::write(&file_path, &bytes);
                                        println!("[Cover Pipeline] Capa salva no cache: {:?}", file_path);
                                        Some(bytes.to_vec())
                                    } else {
                                        println!("[Cover Pipeline] Falha ao extrair bytes da resposta HTTP!");
                                        None
                                    }
                                }
                                Err(e) => {
                                    println!("[Cover Pipeline] Erro na requisição HTTP reqwest: {:?}", e);
                                    None
                                }
                            }
                        };

                        if let Some(bytes) = bytes_opt {
                            println!("[Cover Pipeline] Imagem de {} bytes obtida. Decodificando buffers...", bytes.len());
                            let path_for_ui = file_path.clone();

                            // Processamento de Ambient Art e Cover em thread pool secundária
                            let (cover_buf_opt, ambient_buf_opt) = tokio::task::spawn_blocking(move || {
                                if let Ok(dynamic_img) = image::load_from_memory(&bytes) {
                                    // 1. Capa nítida para o display central
                                    let rgba_cov = dynamic_img.to_rgba8();
                                    let (cw, ch) = rgba_cov.dimensions();
                                    let mut c_buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(cw, ch);
                                    c_buf.make_mut_bytes().copy_from_slice(rgba_cov.as_raw());

                                    // 2. Downscale rápido para 80x80 e desfoque para Ambient Art
                                    let small = dynamic_img.thumbnail(80, 80);
                                    let blurred = small.blur(14.0);
                                    let rgba_amb = blurred.to_rgba8();
                                    let (aw, ah) = rgba_amb.dimensions();
                                    let mut a_buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(aw, ah);
                                    a_buf.make_mut_bytes().copy_from_slice(rgba_amb.as_raw());

                                    (Some(c_buf), Some(a_buf))
                                } else {
                                    println!("[Cover Pipeline] Falha ao decodificar imagem com image::load_from_memory");
                                    (None, None)
                                }
                            }).await.unwrap_or((None, None));

                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(app) = app_w_cov.upgrade() {
                                    if let Some(c_buf) = cover_buf_opt {
                                        println!("[Cover Pipeline] Aplicando cover_buffer no Slint via from_rgba8!");
                                        let cover_img = slint::Image::from_rgba8(c_buf);
                                        app.set_current_track_cover(cover_img.clone());
                                        app.set_has_cover(true);
                                        RECENT_TRACKS_MODEL.with(|m| {
                                            if let Some(ref model) = *m.borrow() {
                                                if let Some(mut first) = model.row_data(0) {
                                                    first.cover = cover_img;
                                                    first.has_cover = true;
                                                    model.set_row_data(0, first);
                                                }
                                            }
                                        });
                                    } else if let Ok(img) = slint::Image::load_from_path(&path_for_ui) {
                                        println!("[Cover Pipeline] Fallback load_from_path aplicado!");
                                        app.set_current_track_cover(img.clone());
                                        app.set_has_cover(true);
                                        RECENT_TRACKS_MODEL.with(|m| {
                                            if let Some(ref model) = *m.borrow() {
                                                if let Some(mut first) = model.row_data(0) {
                                                    first.cover = img;
                                                    first.has_cover = true;
                                                    model.set_row_data(0, first);
                                                }
                                            }
                                        });
                                    }

                                    if let Some(a_buf) = ambient_buf_opt {
                                        println!("[Cover Pipeline] Aplicando ambient_art no Slint via from_rgba8!");
                                        app.set_background_ambient_art(slint::Image::from_rgba8(a_buf));
                                        app.set_has_ambient_art(true);
                                    }
                                }
                            });
                        } else {
                            println!("[Cover Pipeline] Nenhum dado de imagem disponível para a faixa.");
                        }
                    }
                });
            } else {
                println!("[Cover Pipeline] Faixa sem cover_url. Limpando exibição de capa.");
                if let Some(app) = app_weak.upgrade() {
                    app.set_has_cover(false);
                    app.set_has_ambient_art(false);
                }
            }

            // Dispara busca assíncrona de letras no LRCLIB
            let app_w = app_weak.clone();
            let q_state = queue_state.clone();
            let artist = track.artist.clone();
            let title = track.title.clone();
            let album = Some(track.album.clone());
            let duration_s = Some(track.duration_ms as f64 / 1000.0);

            tokio::spawn(async move {
                match api::lyrics::fetch_synced_lyrics(&artist, &title, album.as_deref(), duration_s).await {
                    Ok(Some(lrc_text)) => {
                        let parsed = models::parse_lrc(&lrc_text);
                        let lyric_items: Vec<LyricItem> = parsed
                            .iter()
                            .enumerate()
                            .map(|(i, l)| LyricItem {
                                text: l.text.clone().into(),
                                timestamp_ms: l.timestamp_ms as i32,
                                is_active: i == 0,
                            })
                            .collect();

                        let app_w_inner = app_w.clone();
                        let q_state_inner = q_state.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            let mut state = q_state_inner.lock().unwrap();
                            state.lyrics = parsed;
                            state.active_lyric_idx = Some(0);
                            if let Some(app) = app_w_inner.upgrade() {
                                app.set_active_lyric_idx(0);
                                app.set_lyrics(ModelRc::from(Rc::new(VecModel::from(lyric_items))));
                            }
                        });
                    }
                    _ => {
                        info!("Nenhuma letra encontrada para a faixa atual.");
                        let app_w_inner = app_w.clone();
                        let q_state_inner = q_state.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            let mut state = q_state_inner.lock().unwrap();
                            state.lyrics.clear();
                            state.active_lyric_idx = None;
                            if let Some(app) = app_w_inner.upgrade() {
                                app.set_active_lyric_idx(0);
                                app.set_lyrics(ModelRc::from(Rc::new(VecModel::from(vec![
                                    LyricItem {
                                        text: "Letras não disponíveis para esta faixa".into(),
                                        timestamp_ms: 0,
                                        is_active: true,
                                    }
                                ]))));
                            }
                        });
                    }
                }
            });
        })
    };

    // Callback: Play/Pause (Não bloqueante e seguro)
    let queue_state_play1 = queue_state.clone();
    let audio_cmd_tx_play1 = audio_cmd_tx.clone();
    let app_weak_play1 = app_weak.clone();
    let play_fn_toggle1 = play_track_fn.clone();
    app.on_toggle_play_pause(move || {
        println!("[DEBUG] Botão Play/Pause clicado na UI!");
        info!("[TopPlayer] Botão Play/Pause (toggle_play_pause) acionado!");
        let state = queue_state_play1.lock().unwrap();
        if state.tracks.is_empty() {
            drop(state);
            if let Some(app) = app_weak_play1.upgrade() {
                let recents = app.get_recent_tracks();
                let mut recent_tracks = Vec::new();
                for i in 0..recents.row_count() {
                    if let Some(item) = recents.row_data(i) {
                        recent_tracks.push(models::Track {
                            id: item.id.to_string(),
                            uri: item.uri.to_string(),
                            title: item.title.to_string(),
                            artist: item.artist.to_string(),
                            album: item.album.to_string(),
                            duration_ms: item.duration_ms as u32,
                            cover_url: if item.cover_url.is_empty() { None } else { Some(item.cover_url.to_string()) },
                        });
                    }
                }
                if !recent_tracks.is_empty() {
                    let track = recent_tracks[0].clone();
                    println!("[DEBUG] Fila vazia, iniciando primeira faixa recente: {}", track.title);
                    let mut st = queue_state_play1.lock().unwrap();
                    st.tracks = recent_tracks;
                    st.current_idx = 0;
                    st.last_played_uri = None;
                    drop(st);
                    play_fn_toggle1(track);
                    return;
                }
            }
            println!("[DEBUG] Nenhuma música selecionada e nenhuma recente disponível.");
            info!("Nenhuma música selecionada para tocar.");
            return;
        }
        if let Some(app) = app_weak_play1.upgrade() {
            let current = app.get_is_playing();
            let new_state = !current;
            println!("[DEBUG] Alternando reprodução: is_playing atual = {}, novo = {}", current, new_state);
            app.set_is_playing(new_state);
        }
        let res = audio_cmd_tx_play1.try_send(audio::player::PlayerCommand::Toggle);
        println!("[DEBUG] audio_cmd_tx Toggle enviado, resultado: {:?}", res);
    });

    app.on_play_pause(move || {
        // Alias de compatibilidade Slint (o fluxo principal é roteado via on_toggle_play_pause)
    });

    // Callback: Próxima Faixa
    let queue_state_next = queue_state.clone();
    let play_fn_next = play_track_fn.clone();
    let app_weak_next = app_weak.clone();
    app.on_next_track(move || {
        println!("[DEBUG] Next track disparado");
        println!("[DEBUG] Botão Próxima Faixa (next_track) clicado na UI!");
        info!("[TopPlayer] Botão Próxima Faixa (next_track) acionado!");

        // 1. Se houver faixas na fila "A Seguir" (QUEUE_TRACKS_MODEL), consome e toca a primeira
        let next_queued_track: Option<models::Track> = QUEUE_TRACKS_MODEL.with(|m| {
            if let Some(ref model) = *m.borrow() {
                if model.row_count() > 0 {
                    if let Some(item) = model.row_data(0) {
                        return Some(models::Track {
                            id: item.id.to_string(),
                            uri: item.uri.to_string(),
                            title: item.title.to_string(),
                            artist: item.artist.to_string(),
                            album: item.album.to_string(),
                            duration_ms: item.duration_ms as u32,
                            cover_url: if item.cover_url.is_empty() { None } else { Some(item.cover_url.to_string()) },
                        });
                    }
                }
            }
            None
        });

        if let Some(queued_track) = next_queued_track {
            println!("[NextTrack] Tocando próxima faixa da fila 'A Seguir': {}", queued_track.title);
            let mut state = queue_state_next.lock().unwrap();
            state.last_played_uri = None;
            let insert_pos = (state.current_idx + 1).min(state.tracks.len());
            state.tracks.insert(insert_pos, queued_track.clone());
            state.current_idx = insert_pos;
            drop(state);
            play_fn_next(queued_track);
            return;
        }

        let mut state = queue_state_next.lock().unwrap();
        state.last_played_uri = None;
        if state.tracks.is_empty() {
            drop(state);
            if let Some(app) = app_weak_next.upgrade() {
                let recents = app.get_recent_tracks();
                let mut recent_tracks = Vec::new();
                for i in 0..recents.row_count() {
                    if let Some(item) = recents.row_data(i) {
                        recent_tracks.push(models::Track {
                            id: item.id.to_string(),
                            uri: item.uri.to_string(),
                            title: item.title.to_string(),
                            artist: item.artist.to_string(),
                            album: item.album.to_string(),
                            duration_ms: item.duration_ms as u32,
                            cover_url: if item.cover_url.is_empty() { None } else { Some(item.cover_url.to_string()) },
                        });
                    }
                }
                if !recent_tracks.is_empty() {
                    let track = recent_tracks[0].clone();
                    println!("[DEBUG] Fila vazia ao clicar Próxima, iniciando primeira recente: {}", track.title);
                    let mut st = queue_state_next.lock().unwrap();
                    st.tracks = recent_tracks;
                    st.current_idx = 0;
                    st.last_played_uri = None;
                    drop(st);
                    play_fn_next(track);
                }
            }
            return;
        }
        if state.current_idx + 1 < state.tracks.len() {
            state.current_idx += 1;
            let track = state.tracks[state.current_idx].clone();
            println!("[DEBUG] Avançando para faixa [{}]: {}", state.current_idx, track.title);
            drop(state);
            play_fn_next(track);
        } else {
            // Loop ao início da lista
            state.current_idx = 0;
            let track = state.tracks[0].clone();
            println!("[DEBUG] Fim da fila alcançado. Reiniciando lista no índice 0: {}", track.title);
            drop(state);
            play_fn_next(track);
        }
    });

    // Callback: Faixa Anterior
    let queue_state_prev = queue_state.clone();
    let play_fn_prev = play_track_fn.clone();
    let audio_cmd_tx_prev = audio_cmd_tx.clone();
    let handle_prev = move || {
        println!("[DEBUG] Previous track disparado");
        println!("[DEBUG] Botão Faixa Anterior (previous_track) clicado na UI!");
        info!("[TopPlayer] Botão Faixa Anterior (previous_track) acionado!");
        let mut state = queue_state_prev.lock().unwrap();
        state.last_played_uri = None;
        if state.current_idx > 0 {
            state.current_idx -= 1;
            let track = state.tracks[state.current_idx].clone();
            println!("[DEBUG] Voltando para faixa [{}]: {}", state.current_idx, track.title);
            drop(state);
            play_fn_prev(track);
        } else if !state.tracks.is_empty() {
            let _ = audio_cmd_tx_prev.try_send(audio::player::PlayerCommand::Seek(0));
            let track = state.tracks[0].clone();
            println!("[DEBUG] Já na primeira faixa, reiniciando reprodução em 0:00: {}", track.title);
            drop(state);
            play_fn_prev(track);
        }
    };

    let handle_prev_clone = handle_prev.clone();
    app.on_previous_track(handle_prev);
    app.on_prev_track(handle_prev_clone);

    // Callback: Seek no Scrubber
    let audio_cmd_tx_seek = audio_cmd_tx.clone();
    let queue_state_seek = queue_state.clone();
    app.on_seek(move |fraction| {
        info!("[TopPlayer] Seek acionado: fraction={}", fraction);
        let state = queue_state_seek.lock().unwrap();
        if let Some(track) = state.tracks.get(state.current_idx) {
            let target_ms = ((fraction * track.duration_ms as f32) as u32).clamp(0, track.duration_ms);
            let _ = audio_cmd_tx_seek.try_send(audio::player::PlayerCommand::Seek(target_ms));
        }
    });

    app.on_seek_progress(move |_| {
        // Alias de compatibilidade Slint (o fluxo principal é roteado via on_seek)
    });

    // Callback: Clicar na Letra para saltar (Karaokê interativo)
    let audio_cmd_tx_lyric_seek = audio_cmd_tx.clone();
    app.on_seek_lyric(move |timestamp_ms| {
        if timestamp_ms >= 0 {
            let _ = audio_cmd_tx_lyric_seek.try_send(audio::player::PlayerCommand::Seek(timestamp_ms as u32));
        }
    });

    // Callback: Controle de Volume (0.0 a 1.0)
    let audio_cmd_tx_vol = audio_cmd_tx.clone();
    let app_weak_vol = app_weak.clone();
    app.on_set_volume(move |vol| {
        let clamped = vol.clamp(0.0, 1.0);
        if let Some(app) = app_weak_vol.upgrade() {
            app.set_volume(clamped);
        }
        let _ = audio_cmd_tx_vol.try_send(audio::player::PlayerCommand::SetVolume(clamped));
        // Persiste o volume ajustado
        let mut cfg = utils::config::AppConfig::load();
        cfg.volume = clamped;
        cfg.save();
    });

    // Callback: Reabrir Navegador se o usuário clicar no banner
    let current_auth_url_open = current_auth_url.clone();
    app.on_open_auth_browser(move || {
        let url = current_auth_url_open.lock().unwrap().clone();
        if !url.is_empty() {
            let _ = webbrowser::open(&url);
        }
    });

    // Callback: Alternar visão de letras
    let app_weak_lyrics = app_weak.clone();
    app.on_toggle_lyrics(move || {
        if let Some(app) = app_weak_lyrics.upgrade() {
            let is_lyrics = app.get_current_view() == "lyrics";
            if is_lyrics {
                app.set_current_view("listen_now".into());
                app.set_lyrics_active(false);
            } else {
                app.set_current_view("lyrics".into());
                app.set_lyrics_active(true);
            }
        }
    });

    // Callback: Alternar Drawer da Fila de Reprodução (Up Next)
    let app_weak_queue = app_weak.clone();
    let access_token_queue = access_token_shared.clone();
    let queue_state_toggle = queue_state.clone();
    app.on_toggle_queue(move || {
        if let Some(app) = app_weak_queue.upgrade() {
            let current = app.get_queue_open();
            let new_state = !current;
            println!("[DEBUG] Alternando Fila de Reprodução: queue_open = {}", new_state);
            app.set_queue_open(new_state);

            if new_state {
                // 1. Imediatamente exibe as faixas da fila local como fallback rápido
                let q_state = queue_state_toggle.clone();
                let state = q_state.lock().unwrap();
                let current_playing_uri = state.last_played_uri.clone();
                let local_upcoming: Vec<models::Track> = if state.current_idx + 1 < state.tracks.len() {
                    state.tracks[state.current_idx + 1..]
                        .iter()
                        .filter(|t| current_playing_uri.as_ref().map_or(true, |u| u != &t.uri))
                        .cloned()
                        .collect()
                } else {
                    Vec::new()
                };
                drop(state);

                let local_items: Vec<TrackItem> = local_upcoming
                    .iter()
                    .map(|t| {
                        let duration = t.formatted_duration();
                        TrackItem {
                            id: t.id.clone().into(),
                            uri: t.uri.clone().into(),
                            title: t.title.clone().into(),
                            artist: t.artist.clone().into(),
                            album: t.album.clone().into(),
                            duration: duration.into(),
                            duration_ms: t.duration_ms as i32,
                            cover_url: t.cover_url.clone().unwrap_or_default().into(),
                            cover: slint::Image::default(),
                            has_cover: false,
                        }
                    })
                    .collect();

                let model = Rc::new(VecModel::from(local_items));
                QUEUE_TRACKS_MODEL.with(|m| {
                    *m.borrow_mut() = Some(model.clone());
                });
                app.set_queue_tracks(ModelRc::from(model));

                // Baixa capas da fila local caso existam
                let local_tracks_covers = local_upcoming.clone();
                tokio::spawn(async move {
                    for (idx, t) in local_tracks_covers.into_iter().enumerate() {
                        if let Some(url) = t.cover_url {
                            if !url.is_empty() {
                                if let Some(buf) = load_image_cover_buffer(&url).await {
                                    let _ = slint::invoke_from_event_loop(move || {
                                        QUEUE_TRACKS_MODEL.with(|m| {
                                            if let Some(ref model) = *m.borrow() {
                                                if let Some(mut item) = model.row_data(idx) {
                                                    item.cover = slint::Image::from_rgba8(buf);
                                                    item.has_cover = true;
                                                    model.set_row_data(idx, item);
                                                }
                                            }
                                        });
                                    });
                                }
                            }
                        }
                    }
                });

                // 2. Consulta a API do Spotify em background para obter a fila real
                let token_lock = access_token_queue.clone();
                let app_w = app_weak_queue.clone();
                let q_state_sp = queue_state_toggle.clone();
                tokio::spawn(async move {
                    let token = token_lock.lock().unwrap().clone();
                    if !token.is_empty() {
                        if let Ok(spotify_queue) = api::client::fetch_user_queue(&token).await {
                            let curr_uri = q_state_sp.lock().unwrap().last_played_uri.clone();
                            let spotify_queue: Vec<models::Track> = spotify_queue
                                .into_iter()
                                .filter(|t| curr_uri.as_ref().map_or(true, |u| u != &t.uri))
                                .collect();

                            if !spotify_queue.is_empty() {
                                let queue_for_ui = spotify_queue.clone();
                                let app_w_inner = app_w.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    let items: Vec<TrackItem> = queue_for_ui
                                        .iter()
                                        .map(|t| {
                                             let duration = t.formatted_duration();
                                             TrackItem {
                                                 id: t.id.clone().into(),
                                                 uri: t.uri.clone().into(),
                                                 title: t.title.clone().into(),
                                                 artist: t.artist.clone().into(),
                                                 album: t.album.clone().into(),
                                                 duration: duration.into(),
                                                 duration_ms: t.duration_ms as i32,
                                                 cover_url: t.cover_url.clone().unwrap_or_default().into(),
                                                 cover: slint::Image::default(),
                                                 has_cover: false,
                                             }
                                         })
                                        .collect();

                                    let model = Rc::new(VecModel::from(items));
                                    QUEUE_TRACKS_MODEL.with(|m| {
                                        *m.borrow_mut() = Some(model.clone());
                                    });
                                    if let Some(app) = app_w_inner.upgrade() {
                                        app.set_queue_tracks(ModelRc::from(model));
                                    }
                                });

                                // Baixa capas da fila do Spotify em background
                                tokio::spawn(async move {
                                    for (idx, t) in spotify_queue.into_iter().enumerate() {
                                        if let Some(url) = t.cover_url {
                                            if !url.is_empty() {
                                                if let Some(buf) = load_image_cover_buffer(&url).await {
                                                    let _ = slint::invoke_from_event_loop(move || {
                                                        QUEUE_TRACKS_MODEL.with(|m| {
                                                            if let Some(ref model) = *m.borrow() {
                                                                if let Some(mut item) = model.row_data(idx) {
                                                                    item.cover = slint::Image::from_rgba8(buf);
                                                                    item.has_cover = true;
                                                                    model.set_row_data(idx, item);
                                                                }
                                                            }
                                                        });
                                                    });
                                                }
                                            }
                                        }
                                    }
                                });
                            }
                        }
                    }
                });
            }
        }
    });

    // Callback: Tocar faixa a partir da fila de reprodução (Up Next)
    let app_weak_queue_play = app_weak.clone();
    let queue_state_queue_play = queue_state.clone();
    let play_fn_queue = play_track_fn.clone();
    app.on_play_queue_track(move |idx| {
        let index = idx as usize;
        println!("[DEBUG] Faixa da fila selecionada no índice: {}", index);
        if let Some(app) = app_weak_queue_play.upgrade() {
            let model = app.get_queue_tracks();
            if let Some(item) = model.row_data(index) {
                let track = models::Track {
                    id: item.id.to_string(),
                    uri: item.uri.to_string(),
                    title: item.title.to_string(),
                    artist: item.artist.to_string(),
                    album: item.album.to_string(),
                    duration_ms: item.duration_ms as u32,
                    cover_url: if item.cover_url.is_empty() { None } else { Some(item.cover_url.to_string()) },
                };
                
                let mut state = queue_state_queue_play.lock().unwrap();
                state.last_played_uri = None;
                let found_idx = state.tracks.iter().position(|t| t.id == track.id);
                if let Some(pos) = found_idx {
                    state.current_idx = pos;
                } else {
                    let insert_pos = (state.current_idx + 1).min(state.tracks.len());
                    state.tracks.insert(insert_pos, track.clone());
                    state.current_idx = insert_pos;
                }
                drop(state);
                play_fn_queue(track);
            }
        }
    });

    // Callback: Navegação da Sidebar
    let app_weak_nav = app_weak.clone();
    let access_token_nav = access_token_shared.clone();
    let queue_state_nav = queue_state.clone();
    app.on_navigate(move |view_name| {
        let view = view_name.to_string();

        // Persiste a navegação se não for letras
        if view != "lyrics" {
            let mut cfg = utils::config::AppConfig::load();
            cfg.last_view = view.clone();
            cfg.save();
        }

        if let Some(app) = app_weak_nav.upgrade() {
            app.set_active_nav(view.clone().into());
            app.set_lyrics_active(false);

            if view == "liked" {
                app.set_current_view("playlist_detail".into());
                app.set_playlist_title("Músicas Curtidas".into());
                app.set_playlist_subtitle("Suas faixas favoritas salvas no Spotify".into());
                app.set_playlist_track_count(0);
                app.set_playlist_has_cover(false);
                app.set_current_tracks(ModelRc::from(Rc::new(VecModel::from(Vec::<TrackItem>::new()))));

                let app_w = app_weak_nav.clone();
                let token_lock = access_token_nav.clone();
                let q_state = queue_state_nav.clone();

                tokio::spawn(async move {
                    let token = token_lock.lock().unwrap().clone();
                    if !token.is_empty() {
                        if let Ok(tracks) = api::client::fetch_liked_tracks(&token, 50).await {
                            let track_count = tracks.len() as i32;
                            let tracks_clone = tracks.clone();
                            let app_w_inner = app_w.clone();
                            let q_state_inner = q_state.clone();

                            let _ = slint::invoke_from_event_loop(move || {
                                let mut state = q_state_inner.lock().unwrap();
                                state.tracks = tracks_clone.clone();
                                state.current_idx = 0;

                                let items: Vec<TrackItem> = tracks_clone
                                    .iter()
                                    .map(|t| TrackItem {
                                        id: t.id.clone().into(),
                                        uri: t.uri.clone().into(),
                                        title: t.title.clone().into(),
                                        artist: t.artist.clone().into(),
                                        album: t.album.clone().into(),
                                        duration: t.formatted_duration().into(),
                                        duration_ms: t.duration_ms as i32,
                                        cover_url: t.cover_url.clone().unwrap_or_default().into(),
                                        cover: slint::Image::default(),
                                        has_cover: false,
                                    })
                                    .collect();

                                if let Some(app) = app_w_inner.upgrade() {
                                    app.set_playlist_track_count(track_count);
                                    app.set_current_tracks(ModelRc::from(Rc::new(VecModel::from(items))));
                                }
                            });
                        }
                    }
                });
            } else {
                app.set_current_view(view.into());
            }
        }
    });

    // Callback: Selecionar Playlist da Sidebar
    let app_weak_pl = app_weak.clone();
    let access_token_pl = access_token_shared.clone();
    let queue_state_pl = queue_state.clone();
    app.on_select_playlist(move |playlist_id| {
        let app_w = app_weak_pl.clone();
        let token_lock = access_token_pl.clone();
        let q_state = queue_state_pl.clone();
        let p_id = playlist_id.to_string();

        if let Some(app) = app_w.upgrade() {
            app.set_active_nav(p_id.clone().into());
            app.set_current_view("playlist_detail".into());
            app.set_lyrics_active(false);

            // Procura o nome e a capa da playlist na lista já carregada para exibir imediatamente
            let playlists = app.get_playlists();
            let mut found_title = "Playlist".to_string();
            let mut found_count = 0;
            let mut found_cover = slint::Image::default();
            let mut found_has_cover = false;
            let mut found_image_url = String::new();

            for i in 0..playlists.row_count() {
                if let Some(p) = playlists.row_data(i) {
                    if p.id == p_id.as_str() {
                        found_title = p.name.to_string();
                        found_count = p.track_count;
                        found_cover = p.cover;
                        found_has_cover = p.has_cover;
                        found_image_url = p.image_url.to_string();
                        break;
                    }
                }
            }
            app.set_playlist_title(found_title.into());
            app.set_playlist_subtitle("Playlist do Spotify".into());
            app.set_playlist_track_count(found_count);
            app.set_playlist_cover(found_cover);
            app.set_playlist_has_cover(found_has_cover);
            app.set_current_tracks(ModelRc::from(Rc::new(VecModel::from(Vec::<TrackItem>::new()))));

            if !found_has_cover && !found_image_url.is_empty() {
                let app_w_cov = app_w.clone();
                let url_to_fetch = found_image_url.clone();
                tokio::spawn(async move {
                    if let Some(buf) = load_image_cover_buffer(&url_to_fetch).await {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(app) = app_w_cov.upgrade() {
                                app.set_playlist_cover(slint::Image::from_rgba8(buf));
                                app.set_playlist_has_cover(true);
                            }
                        });
                    }
                });
            }
        }

        tokio::spawn(async move {
            let token = token_lock.lock().unwrap().clone();
            if !token.is_empty() {
                info!("Buscando faixas da playlist {}...", p_id);
                match api::client::fetch_playlist_tracks(&token, &p_id).await {
                    Ok(tracks) => {
                        let track_count = tracks.len() as i32;
                        let tracks_clone = tracks.clone();
                        let app_w_inner = app_w.clone();
                        let q_state_inner = q_state.clone();

                        let _ = slint::invoke_from_event_loop(move || {
                            let mut state = q_state_inner.lock().unwrap();
                            state.tracks = tracks_clone.clone();
                            state.current_idx = 0;

                            let items: Vec<TrackItem> = tracks_clone
                                .iter()
                                .map(|t| TrackItem {
                                    id: t.id.clone().into(),
                                    uri: t.uri.clone().into(),
                                    title: t.title.clone().into(),
                                    artist: t.artist.clone().into(),
                                    album: t.album.clone().into(),
                                    duration: t.formatted_duration().into(),
                                    duration_ms: t.duration_ms as i32,
                                    cover_url: t.cover_url.clone().unwrap_or_default().into(),
                                    cover: slint::Image::default(),
                                    has_cover: false,
                                })
                                .collect();

                            if let Some(app) = app_w_inner.upgrade() {
                                app.set_playlist_track_count(track_count);
                                app.set_current_tracks(ModelRc::from(Rc::new(VecModel::from(items))));
                            }
                        });
                    }
                    Err(e) => {
                        log::error!("Erro ao carregar faixas da playlist {}: {}", p_id, e);
                    }
                }
            }
        });
    });

    // Callback: Tocar faixa de uma playlist
    let queue_state_play = queue_state.clone();
    let play_fn_table = play_track_fn.clone();
    app.on_play_track_at_index(move |idx| {
        let mut state = queue_state_play.lock().unwrap();
        state.last_played_uri = None;
        let index = idx as usize;
        if index < state.tracks.len() {
            state.current_idx = index;
            let track = state.tracks[index].clone();
            drop(state);
            play_fn_table(track);
        }
    });

    // Callback: Tocar faixa do Ouvir Agora (Recentes)
    let app_weak_recent = app_weak.clone();
    let queue_state_recent = queue_state.clone();
    let play_fn_recent = play_track_fn.clone();
    app.on_play_recent_track(move |idx| {
        let index = idx as usize;
        if let Some(app) = app_weak_recent.upgrade() {
            let model = app.get_recent_tracks();
            let mut recent_tracks = Vec::new();
            for i in 0..model.row_count() {
                if let Some(item) = model.row_data(i) {
                    recent_tracks.push(models::Track {
                        id: item.id.to_string(),
                        uri: item.uri.to_string(),
                        title: item.title.to_string(),
                        artist: item.artist.to_string(),
                        album: item.album.to_string(),
                        duration_ms: item.duration_ms as u32,
                        cover_url: if item.cover_url.is_empty() { None } else { Some(item.cover_url.to_string()) },
                    });
                }
            }
            if index < recent_tracks.len() {
                let track = recent_tracks[index].clone();
                let mut state = queue_state_recent.lock().unwrap();
                state.last_played_uri = None;
                state.tracks = recent_tracks;
                state.current_idx = index;
                drop(state);
                play_fn_recent(track);
            }
        }
    });

    // Callback: Busca no Spotify (com debounce e controle de concorrência)
    let app_weak_search = app_weak.clone();
    let access_token_search = access_token_shared.clone();
    let search_generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    app.on_search_changed(move |query| {
        let q = query.to_string();
        let app_w = app_weak_search.clone();

        if q.trim().is_empty() {
            if let Some(app) = app_w.upgrade() {
                app.set_current_view("listen_now".into());
                app.set_active_nav("listen_now".into());
                app.set_search_tracks(ModelRc::from(Rc::new(VecModel::from(Vec::<TrackItem>::new()))));
            }
            return;
        }

        let token_lock = access_token_search.clone();
        let gen = search_generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let gen_clone = search_generation.clone();

        if let Some(app) = app_w.upgrade() {
            app.set_current_view("search".into());
            app.set_active_nav("".into());
        }

        tokio::spawn(async move {
            // Debounce de 250ms para aguardar término da digitação
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if gen_clone.load(std::sync::atomic::Ordering::SeqCst) != gen {
                return;
            }

            let token = token_lock.lock().unwrap().clone();
            if !token.is_empty() {
                info!("Executando busca no Spotify por: '{}'", q);
                if let Ok(tracks) = api::client::search_tracks(&token, &q).await {
                    if gen_clone.load(std::sync::atomic::Ordering::SeqCst) != gen {
                        return;
                    }

                    let app_w_inner = app_w.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let items: Vec<TrackItem> = tracks
                            .into_iter()
                            .map(|t| {
                                let duration = t.formatted_duration();
                                TrackItem {
                                    id: t.id.into(),
                                    uri: t.uri.into(),
                                    title: t.title.into(),
                                    artist: t.artist.into(),
                                    album: t.album.into(),
                                    duration: duration.into(),
                                    duration_ms: t.duration_ms as i32,
                                    cover_url: t.cover_url.unwrap_or_default().into(),
                                    cover: slint::Image::default(),
                                    has_cover: false,
                                }
                            })
                            .collect();

                        if let Some(app) = app_w_inner.upgrade() {
                            app.set_search_tracks(ModelRc::from(Rc::new(VecModel::from(items))));
                        }
                    });
                }
            }
        });
    });

    // Callback: Tocar faixa dos resultados de busca
    let app_weak_search_play = app_weak.clone();
    let queue_state_search = queue_state.clone();
    let play_fn_search = play_track_fn.clone();
    app.on_play_search_track(move |idx| {
        let index = idx as usize;
        if let Some(app) = app_weak_search_play.upgrade() {
            let model = app.get_search_tracks();
            let mut search_tracks = Vec::new();
            for i in 0..model.row_count() {
                if let Some(item) = model.row_data(i) {
                    search_tracks.push(models::Track {
                        id: item.id.to_string(),
                        uri: item.uri.to_string(),
                        title: item.title.to_string(),
                        artist: item.artist.to_string(),
                        album: item.album.to_string(),
                        duration_ms: item.duration_ms as u32,
                        cover_url: if item.cover_url.is_empty() { None } else { Some(item.cover_url.to_string()) },
                    });
                }
            }
            if index < search_tracks.len() {
                let track = search_tracks[index].clone();
                let mut state = queue_state_search.lock().unwrap();
                state.last_played_uri = None;
                state.tracks = search_tracks;
                state.current_idx = index;
                drop(state);
                play_fn_search(track);
            }
        }
    });

    // Callback: Compartilhar Faixa
    let queue_state_share = queue_state.clone();
    app.on_share_track(move || {
        let state = queue_state_share.lock().unwrap();
        if let Some(track) = state.tracks.get(state.current_idx) {
            let url = format!("https://open.spotify.com/track/{}", track.id);
            info!("Link da música copiado: {}", url);
        }
    });

    info!("Inicializando loop principal da interface Slint...");
    app.run()?;

    Ok(())
}
