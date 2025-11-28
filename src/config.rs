use std::fs;
use std::collections::HashMap;

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
    pub fn load() -> Self {
        let contents = fs::read_to_string("config.conf")
            .expect("Failed to read config.conf");

        let mut map = HashMap::new();

        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                map.insert(
                    k.trim().to_string(),
                    v.trim().trim_matches('"').to_string(),
                );
            }
        }

        Config {
            navidrome_url: map.get("NAVIDROME_URL").unwrap().into(),
            navidrome_user: map.get("NAVIDROME_USER").unwrap().into(),
            navidrome_token: map.get("NAVIDROME_TOKEN").unwrap().into(),
            navidrome_salt: map.get("NAVIDROME_SALT").unwrap().into(),

            refresh_interval: map.get("REFRESH_INTERVAL")
                .and_then(|v| v.parse().ok())
                .unwrap_or(2),

            karaoke_enabled: map.get("KARAOKE_ENABLED")
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(true),
        }
    }
}
