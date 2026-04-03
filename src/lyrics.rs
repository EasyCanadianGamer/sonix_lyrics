use reqwest::blocking::{Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LyricsError {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Not found")]
    NotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KaraokeWord {
    pub time_ms: u32,
    pub word: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncedLine {
    pub time_ms: u32,
    pub text: String,
    pub words: Vec<KaraokeWord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricsData {
    pub lines: Vec<String>,
    pub synced: Vec<SyncedLine>,
}

#[derive(Debug, Deserialize)]
struct LrcLibResult {
    #[serde(rename = "trackName")]
    track: String,

    #[serde(rename = "artistName")]
    artist: String,

    #[serde(rename = "plainLyrics")]
    plain: Option<String>,

    #[serde(rename = "syncedLyrics")]
    synced: Option<String>,
}

fn http() -> Client {
    ClientBuilder::new()
        .timeout(Duration::from_secs(4))
        .connect_timeout(Duration::from_secs(2))
        .user_agent("sonix_lyrics")
        .build()
        .unwrap()
}

fn parse_ts(ts: &str) -> Option<u32> {
    let mut parts = ts.split(':');
    let m: u32 = parts.next()?.parse().ok()?;
    let s: f32 = parts.next()?.parse().ok()?;

    Some(m * 60_000 + (s * 1000.0).round() as u32)
}

fn parse_karaoke_words(text: &str) -> Vec<KaraokeWord> {
    let mut out = Vec::new();
    let chars = text.as_bytes();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == b'<' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != b'>' {
                j += 1;
            }
            if j >= chars.len() {
                break;
            }

            let ts = &text[i + 1..j];
            let time = match parse_ts(ts) {
                Some(t) => t,
                None => {
                    i = j + 1;
                    continue;
                }
            };

            let mut k = j + 1;
            while k < chars.len() && chars[k] != b'<' {
                k += 1;
            }

            let w = text[j + 1..k].trim();
            if !w.is_empty() {
                out.push(KaraokeWord {
                    time_ms: time,
                    word: w.to_string(),
                });
            }
            i = k;
        } else {
            i += 1;
        }
    }
    out
}

fn strip_karaoke_tags(text: &str) -> String {
    let mut out = String::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            while i < bytes.len() && bytes[i] != b'>' {
                i += 1;
            }
            i += 1;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn parse_lrc(text: &str) -> Vec<SyncedLine> {
    let mut out = Vec::new();

    for l in text.lines() {
        if !l.starts_with('[') {
            continue;
        }
        let end = match l.find(']') {
            Some(i) => i,
            None => continue,
        };
        let ts = &l[1..end];
        let body = l[end + 1..].trim();

        let t = match parse_ts(ts) {
            Some(v) => v,
            None => continue,
        };

        out.push(SyncedLine {
            time_ms: t,
            text: strip_karaoke_tags(body),
            words: parse_karaoke_words(body),
        });
    }

    out.sort_by_key(|l| l.time_ms);
    out
}

// ---- lyrics cache ----

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
        .collect::<String>()
        .to_lowercase()
}

fn cache_path(artist: &str, title: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let key = format!("{}_{}", sanitize(artist), sanitize(title));
    PathBuf::from(format!("{}/.cache/sonix_lyrics/{}.json", home, key))
}

fn load_cache(artist: &str, title: &str) -> Option<LyricsData> {
    let data = std::fs::read_to_string(cache_path(artist, title)).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_cache(artist: &str, title: &str, ld: &LyricsData) {
    let path = cache_path(artist, title);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string(ld) {
        let _ = std::fs::write(path, json);
    }
}

// ---- lrclib search ----

fn search(q: &str) -> Result<Vec<LrcLibResult>, LyricsError> {
    let url = format!("https://lrclib.net/api/search?q={}", urlencoding::encode(q));
    let resp = http().get(url).send()?.error_for_status()?;
    Ok(resp.json()?)
}

pub fn fetch_lyrics(artist: &str, title: &str) -> Result<LyricsData, LyricsError> {
    if let Some(cached) = load_cache(artist, title) {
        log::debug!("Cache hit for {} - {}", artist, title);
        return Ok(cached);
    }

    let res = search(&format!("{} {}", artist, title))
        .or_else(|_| search(title))?;

    if res.is_empty() {
        return Err(LyricsError::NotFound);
    }

    let best = &res[0];
    log::debug!("Using lrclib result: {} - {}", best.artist, best.track);

    let lines = if let Some(ref p) = best.plain {
        p.lines().map(|s| s.to_string()).collect()
    } else if let Some(ref s) = best.synced {
        s.lines()
            .map(|l| {
                if let Some(i) = l.find(']') {
                    l[i + 1..].trim().to_string()
                } else {
                    l.to_string()
                }
            })
            .collect()
    } else {
        vec![]
    };

    let synced = if let Some(ref s) = best.synced {
        parse_lrc(s)
    } else {
        vec![]
    };

    let ld = LyricsData { lines, synced };
    save_cache(artist, title, &ld);
    Ok(ld)
}
