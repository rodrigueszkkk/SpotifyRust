use anyhow::Result;
use log::{info, warn};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct LrclibResponse {
    pub id: i64,
    pub name: String,
    pub artist_name: String,
    pub album_name: String,
    pub duration: f64,
    pub instrumental: bool,
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>, // LRC format (time-synced)
}

/// Busca a letra sincronizada (LRC) na API aberta do LRCLIB.net
pub async fn fetch_synced_lyrics(
    artist: &str,
    track_name: &str,
    album_name: Option<&str>,
    duration_s: Option<f64>,
) -> Result<Option<String>> {
    info!("Buscando letras em tempo real para: {} - {}", artist, track_name);

    let mut url = reqwest::Url::parse("https://lrclib.net/api/get")?;
    url.query_pairs_mut()
        .append_pair("artist_name", artist)
        .append_pair("track_name", track_name);

    if let Some(album) = album_name {
        url.query_pairs_mut().append_pair("album_name", album);
    }

    if let Some(duration) = duration_s {
        url.query_pairs_mut().append_pair("duration", &duration.to_string());
    }

    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .header(
            "User-Agent",
            "SpotifyRust AppleMusicClone/0.1.0 (https://github.com/spotify-rust)",
        )
        .send()
        .await?;

    if response.status().is_success() {
        let data: LrclibResponse = response.json().await?;
        if let Some(synced) = data.synced_lyrics {
            info!("Letras sincronizadas encontradas!");
            return Ok(Some(synced));
        } else if let Some(plain) = data.plain_lyrics {
            info!("Letras planas (sem sincronia de tempo) encontradas.");
            return Ok(Some(plain));
        }
    } else {
        warn!("Letra não encontrada no LRCLIB (Status: {}).", response.status());
    }

    Ok(None)
}
