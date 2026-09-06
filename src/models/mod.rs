use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u32,
    pub cover_url: Option<String>,
}

impl Track {
    pub fn formatted_duration(&self) -> String {
        let total_seconds = self.duration_ms / 1000;
        let minutes = total_seconds / 60;
        let seconds = total_seconds % 60;
        format!("{}:{:02}", minutes, seconds)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub description: String,
    pub image_url: Option<String>,
    pub tracks_total: u32,
    pub uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricLine {
    pub timestamp_ms: u32,
    pub text: String,
}

/// Faz o parsing de uma string no formato LRC para uma lista de LyricLine ordenadas por tempo
pub fn parse_lrc(content: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('[') {
            continue;
        }

        if let Some(close_bracket) = trimmed.find(']') {
            let time_str = &trimmed[1..close_bracket];
            let text = trimmed[close_bracket + 1..].trim().to_string();

            if let Some(ms) = parse_lrc_timestamp(time_str) {
                if !text.is_empty() {
                    lines.push(LyricLine {
                        timestamp_ms: ms,
                        text,
                    });
                }
            }
        }
    }

    lines.sort_by_key(|l| l.timestamp_ms);
    lines
}

fn parse_lrc_timestamp(timestamp: &str) -> Option<u32> {
    // Formatos comuns: mm:ss.xx ou mm:ss:xx ou mm:ss.xxx
    let parts: Vec<&str> = timestamp.split(':').collect();
    if parts.len() < 2 {
        return None;
    }

    let minutes: u32 = parts[0].parse().ok()?;
    let sec_parts: Vec<&str> = parts[1].split('.').collect();
    let seconds: u32 = sec_parts[0].parse().ok()?;
    let mut ms: u32 = 0;

    if sec_parts.len() > 1 {
        let frac_str = sec_parts[1];
        if frac_str.len() == 2 {
            ms = frac_str.parse::<u32>().ok()? * 10;
        } else if frac_str.len() >= 3 {
            ms = frac_str[..3].parse::<u32>().ok()?;
        }
    }

    Some(minutes * 60 * 1000 + seconds * 1000 + ms)
}
