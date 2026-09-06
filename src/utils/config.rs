use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default = "default_view")]
    pub last_view: String,
}

fn default_volume() -> f32 {
    0.8
}

fn default_view() -> String {
    "listen_now".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            volume: default_volume(),
            last_view: default_view(),
        }
    }
}

impl AppConfig {
    pub fn config_path() -> Option<PathBuf> {
        crate::api::auth::get_data_dir().map(|d| d.join("config.json"))
    }

    pub fn load() -> Self {
        if let Some(path) = Self::config_path() {
            if path.exists() {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(cfg) = serde_json::from_str::<AppConfig>(&content) {
                        return cfg;
                    }
                }
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        if let Some(path) = Self::config_path() {
            if let Ok(json) = serde_json::to_string_pretty(self) {
                let _ = fs::write(path, json);
            }
        }
    }
}
