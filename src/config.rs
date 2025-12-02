// src/config.rs
use std::fs;
use std::path::PathBuf;
use std::collections::HashMap;

use crate::setup::run_setup_wizard;

#[derive(Debug, Clone)]
pub struct Config {
    pub navidrome_url: String,
    pub navidrome_user: String,
    pub navidrome_token: String,
    pub navidrome_salt: String,

    pub refresh_interval: u64,
    pub karaoke_enabled: bool,
}

impl Config {
    fn config_path() -> PathBuf {
        let home = std::env::var("HOME").expect("HOME not set");
        PathBuf::from(format!("{}/.config/sonix_lyrics/config.conf", home))
    }

    pub fn load_or_setup() -> Self {
        let path = Self::config_path();

        // If config missing → run wizard
        if !path.exists() {
            println!("No config found — launching setup wizard...");
            let cfg = run_setup_wizard();
            cfg.save();
            return cfg;
        }

        Self::load()
    }

    pub fn load() -> Self {
        let contents = fs::read_to_string(Self::config_path())
            .expect("Failed to read config.conf");

        let mut map = HashMap::new();

        for line in contents.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') { continue; }

            if let Some((k, v)) = l.split_once('=') {
                map.insert(
                    k.trim().to_string(),
                    v.trim().trim_matches('"').to_string(),
                );
            }
        }

        Config {
            navidrome_url: map.get("NAVIDROME_URL").unwrap().clone(),
            navidrome_user: map.get("NAVIDROME_USER").unwrap().clone(),
            navidrome_token: map.get("NAVIDROME_TOKEN").unwrap().clone(),
            navidrome_salt: map.get("NAVIDROME_SALT").unwrap().clone(),

            refresh_interval: map.get("REFRESH_INTERVAL")
                .and_then(|v| v.parse().ok())
                .unwrap_or(2),

            karaoke_enabled: map.get("KARAOKE_ENABLED")
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        }
    }

    pub fn save(&self) {
        let path = Self::config_path();
        let dir = path.parent().unwrap();
        std::fs::create_dir_all(dir).unwrap();

        let data = format!(
r#"# Sonix Lyrics Config

NAVIDROME_URL = {}
NAVIDROME_USER = {}
NAVIDROME_TOKEN = {}
NAVIDROME_SALT = {}

REFRESH_INTERVAL = {}
KARAOKE_ENABLED = {}
"#,
            self.navidrome_url,
            self.navidrome_user,
            self.navidrome_token,
            self.navidrome_salt,
            self.refresh_interval,
            self.karaoke_enabled,
        );

        fs::write(path, data).expect("Failed to write config file");
    }
}
