use reqwest::blocking::{ClientBuilder};
use serde::Deserialize;
use thiserror::Error;
use chrono::{DateTime, Utc};
use std::time::Duration;

use crate::config::Config;

#[derive(Debug, Error)]
pub enum NavidromeError {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Invalid response")]
    InvalidResponse,

    #[error("No song playing")]
    NoTrack,
}

#[derive(Debug, Clone)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: u32,

    pub played_timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct SubsonicResponse<T> {
    #[serde(rename = "subsonic-response")]
    response: T,
}

#[derive(Debug, Deserialize)]
struct NowPlayingWrapper {
    status: String,
    #[serde(rename = "nowPlaying")]
    now_playing: Option<NowPlaying>,
}

#[derive(Debug, Deserialize)]
struct NowPlaying {
    entry: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    duration: Option<u32>,
    played: Option<String>,
}

pub fn get_current_track(cfg: &Config) -> Result<Track, NavidromeError> {
    let url = format!(
        "{}/rest/getNowPlaying?u={}&t={}&s={}&v=1.16.1&c=sonix&f=json",
        cfg.navidrome_url, cfg.navidrome_user, cfg.navidrome_token, cfg.navidrome_salt
    );

    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(4))
        .connect_timeout(Duration::from_secs(2))
        .build()?;

    let resp = client.get(url).send()?.error_for_status()?;
    let parsed: SubsonicResponse<NowPlayingWrapper> = resp.json()?;

    if parsed.response.status != "ok" {
        return Err(NavidromeError::InvalidResponse);
    }

    let entries = parsed.response.now_playing
        .ok_or(NavidromeError::NoTrack)?
        .entry;

    if entries.is_empty() {
        return Err(NavidromeError::NoTrack);
    }

    let e = &entries[0];
    let played_timestamp = e.played
        .as_ref()
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&Utc));

    Ok(Track {
        title: e.title.clone().unwrap_or_default(),
        artist: e.artist.clone().unwrap_or_default(),
        album: e.album.clone().unwrap_or_default(),
        duration: e.duration.unwrap_or(0),
        played_timestamp,
    })
}
