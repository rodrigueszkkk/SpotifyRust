use anyhow::Result;
use log::{info, warn};
use reqwest::header::AUTHORIZATION;
use reqwest::Client;
use serde_json::Value;

use crate::models::{Playlist, Track};

fn create_client() -> Client {
    Client::builder()
        .user_agent("SpotifyRust AppleMusicClone/0.1.0")
        .build()
        .unwrap_or_else(|_| Client::new())
}

/// Busca as playlists da biblioteca do usuário logado diretamente via API REST
pub async fn fetch_user_playlists(access_token: &str) -> Result<Vec<Playlist>> {
    info!("Buscando playlists do usuário no Spotify...");
    let client = create_client();
    let res = client
        .get("https://api.spotify.com/v1/me/playlists?limit=50")
        .header(AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        warn!("Erro na resposta de playlists do Spotify: {}", err);
        return Ok(Vec::new());
    }

    let json: Value = res.json().await?;
    let mut playlists = Vec::new();

    if let Some(items) = json.get("items").and_then(|v| v.as_array()) {
        for item in items {
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("Sem nome").to_string();
            let uri = item.get("uri").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let description = item.get("description").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            
            let image_url = item
                .get("images")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|img| img.get("url"))
                .and_then(|u| u.as_str())
                .map(|s| s.to_string());

            let tracks_total = item
                .get("tracks")
                .and_then(|v| {
                    if let Some(total) = v.get("total").and_then(|t| t.as_u64()) {
                        Some(total as u32)
                    } else if let Some(total) = v.as_u64() {
                        Some(total as u32)
                    } else {
                        None
                    }
                })
                .unwrap_or(0);

            if !id.is_empty() {
                playlists.push(Playlist {
                    id,
                    name,
                    description,
                    image_url,
                    tracks_total,
                    uri,
                });
            }
        }
    }

    info!("{} playlists reais encontradas com sucesso!", playlists.len());
    Ok(playlists)
}

/// Busca as faixas de uma playlist específica via API REST
pub async fn fetch_playlist_tracks(
    access_token: &str,
    playlist_id: &str,
) -> Result<Vec<Track>> {
    info!("Buscando faixas da playlist {}...", playlist_id);
    let client = create_client();
    // O endpoint /v1/playlists/{id}/tracks retorna 403 no Spotify Developer mode recente.
    // O endpoint principal /v1/playlists/{id} retorna os metadados completos e o objeto de faixas.
    let url = format!("https://api.spotify.com/v1/playlists/{}", playlist_id);
    let res = client
        .get(&url)
        .header(AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        warn!("Erro ao buscar detalhes da playlist {}: {}", playlist_id, err);
        return Ok(Vec::new());
    }

    let json: Value = res.json().await?;
    let mut tracks = Vec::new();

    // Na nova API do Spotify (2024+), os itens podem vir dentro de json["items"]["items"],
    // ou no formato clássico json["tracks"]["items"], ou json["items"] direto.
    let items_array = json
        .get("items")
        .and_then(|v| v.get("items"))
        .and_then(|v| v.as_array())
        .or_else(|| json.get("tracks").and_then(|v| v.get("items")).and_then(|v| v.as_array()))
        .or_else(|| json.get("items").and_then(|v| v.as_array()));

    if let Some(items) = items_array {
        for item in items {
            let track_obj = item.get("item").or_else(|| item.get("track")).unwrap_or(item);
            if let Some(track) = parse_track_json(track_obj) {
                tracks.push(track);
            }
        }
    }

    info!("{} faixas carregadas com sucesso da playlist {}.", tracks.len(), playlist_id);
    Ok(tracks)
}

/// Busca itens curtidos pelo usuário (Músicas Curtidas / Liked Songs)
pub async fn fetch_liked_tracks(
    access_token: &str,
    limit: u32,
) -> Result<Vec<Track>> {
    info!("Buscando faixas curtidas pelo usuário...");
    let client = create_client();
    let url = format!("https://api.spotify.com/v1/me/tracks?limit={}", limit);
    let res = client
        .get(&url)
        .header(AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        warn!("Erro ao buscar faixas curtidas: {}", err);
        return Ok(Vec::new());
    }

    let json: Value = res.json().await?;
    let mut tracks = Vec::new();

    if let Some(items) = json.get("items").and_then(|v| v.as_array()) {
        for item in items {
            let track_obj = item.get("track").or_else(|| item.get("item")).unwrap_or(item);
            if let Some(track) = parse_track_json(track_obj) {
                tracks.push(track);
            }
        }
    }

    info!("{} faixas curtidas carregadas com sucesso!", tracks.len());
    Ok(tracks)
}

/// Busca itens tocados recentemente pelo usuário para a tela Ouvir Agora
pub async fn fetch_recently_played(
    access_token: &str,
    limit: u32,
) -> Result<Vec<Track>> {
    info!("Buscando faixas tocadas recentemente...");
    let client = create_client();
    let url = format!("https://api.spotify.com/v1/me/player/recently-played?limit={}", limit);
    let res = client
        .get(&url)
        .header(AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        warn!("Erro ao buscar faixas recentes: {}", err);
        return Ok(Vec::new());
    }

    let json: Value = res.json().await?;
    let mut tracks = Vec::new();

    if let Some(items) = json.get("items").and_then(|v| v.as_array()) {
        for item in items {
            let track_obj = item.get("track").unwrap_or(item);
            if let Some(track) = parse_track_json(track_obj) {
                tracks.push(track);
            }
        }
    }

    info!("{} faixas recentes encontradas com sucesso!", tracks.len());
    Ok(tracks)
}

/// Pesquisa faixas no catálogo do Spotify
pub async fn search_tracks(
    access_token: &str,
    query: &str,
) -> Result<Vec<Track>> {
    info!("Pesquisando faixas por: '{}'", query);
    let client = create_client();
    let encoded_q = urlencoding::encode(query);
    let url = format!("https://api.spotify.com/v1/search?type=track&limit=10&q={}", encoded_q);
    let res = client
        .get(&url)
        .header(AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        warn!("Erro na busca do Spotify: {}", err);
        return Ok(Vec::new());
    }

    let json: Value = res.json().await?;
    let mut tracks = Vec::new();

    if let Some(items) = json.get("tracks").and_then(|t| t.get("items")).and_then(|v| v.as_array()) {
        for item in items {
            if let Some(track) = parse_track_json(item) {
                tracks.push(track);
            }
        }
    }

    info!("{} faixas encontradas na busca.", tracks.len());
    Ok(tracks)
}

/// Busca a fila de reprodução atual do usuário (Up Next)
pub async fn fetch_user_queue(access_token: &str) -> Result<Vec<Track>> {
    info!("Buscando fila de reprodução do usuário no Spotify...");
    let client = create_client();
    let url = "https://api.spotify.com/v1/me/player/queue";
    let res = client
        .get(url)
        .header(AUTHORIZATION, format!("Bearer {}", access_token))
        .send()
        .await?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        warn!("Erro ao buscar fila do Spotify: {}", err);
        return Ok(Vec::new());
    }

    let json: Value = res.json().await?;
    let mut tracks = Vec::new();

    // Obtém o identificador da música atualmente em reprodução para descartá-la da fila "A Seguir"
    let curr_uri = json.get("currently_playing")
        .and_then(|v| v.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_string();

    let curr_id = json.get("currently_playing")
        .and_then(|v| v.get("id"))
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_string();

    if let Some(items) = json.get("queue").and_then(|v| v.as_array()) {
        for item in items {
            if let Some(track) = parse_track_json(item) {
                // Descarta estritamente a faixa atual caso venha duplicada na fila
                if (!curr_uri.is_empty() && track.uri == curr_uri) || (!curr_id.is_empty() && track.id == curr_id) {
                    continue;
                }
                tracks.push(track);
            }
        }
    }

    info!("{} faixas encontradas na fila do Spotify.", tracks.len());
    Ok(tracks)
}

fn parse_track_json(item: &Value) -> Option<Track> {
    let id = item.get("id").and_then(|v| v.as_str())?.to_string();
    let uri = item.get("uri").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let title = item.get("name").and_then(|v| v.as_str()).unwrap_or("Sem título").to_string();
    let duration_ms = item.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

    let artist = item
        .get("artists")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                .collect::<Vec<&str>>()
                .join(", ")
        })
        .unwrap_or_else(|| "Artista desconhecido".to_string());

    let album_obj = item.get("album");
    let album = album_obj
        .and_then(|a| a.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();

    let cover_url = album_obj
        .and_then(|a| a.get("images"))
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|img| img.get("url"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string());

    Some(Track {
        id,
        uri: if uri.is_empty() { format!("spotify:track:{}", item.get("id").and_then(|v| v.as_str())?) } else { uri },
        title,
        artist,
        album,
        duration_ms,
        cover_url,
    })
}
