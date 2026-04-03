use reqwest::blocking::ClientBuilder;
use serde::Deserialize;
use std::time::Duration;
use thiserror::Error;

use crate::config::Config;

#[derive(Debug, Error)]
pub enum NavidromeError {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Invalid response")]
    InvalidResponse,
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub song_count: u32,
}

#[derive(Debug, Clone)]
pub struct PlaylistTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub duration: u32,
    pub cover_art_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SubsonicResponse<T> {
    #[serde(rename = "subsonic-response")]
    response: T,
}

// ---- serde types for getPlaylists ----

#[derive(Debug, Deserialize)]
struct PlaylistEntry {
    id: String,
    name: String,
    #[serde(rename = "songCount", default)]
    song_count: u32,
}

#[derive(Debug, Deserialize)]
struct PlaylistsWrapper {
    status: String,
    #[serde(rename = "playlists")]
    playlists: PlaylistsInner,
}

#[derive(Debug, Deserialize)]
struct PlaylistsInner {
    #[serde(default)]
    playlist: Vec<PlaylistEntry>,
}

// ---- serde types for getPlaylist ----

#[derive(Debug, Deserialize)]
struct PlaylistTrackEntry {
    id: String,
    title: Option<String>,
    artist: Option<String>,
    duration: Option<u32>,
    #[serde(rename = "coverArt")]
    cover_art: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PlaylistWrapper {
    status: String,
    playlist: PlaylistInner,
}

#[derive(Debug, Deserialize)]
struct PlaylistInner {
    #[serde(default)]
    entry: Vec<PlaylistTrackEntry>,
}

// ---- helpers ----

fn auth_params(cfg: &Config) -> String {
    format!(
        "u={}&t={}&s={}&v=1.16.1&c=sonix&f=json",
        cfg.navidrome_user, cfg.navidrome_token, cfg.navidrome_salt
    )
}

fn make_client() -> Result<reqwest::blocking::Client, NavidromeError> {
    Ok(ClientBuilder::new()
        .timeout(Duration::from_secs(4))
        .connect_timeout(Duration::from_secs(2))
        .build()?)
}

pub fn get_playlists(cfg: &Config) -> Result<Vec<Playlist>, NavidromeError> {
    let url = format!("{}/rest/getPlaylists?{}", cfg.navidrome_url, auth_params(cfg));
    let resp = make_client()?.get(url).send()?.error_for_status()?;
    let parsed: SubsonicResponse<PlaylistsWrapper> = resp.json()?;

    if parsed.response.status != "ok" {
        return Err(NavidromeError::InvalidResponse);
    }

    Ok(parsed.response.playlists.playlist.into_iter().map(|p| Playlist {
        id: p.id,
        name: p.name,
        song_count: p.song_count,
    }).collect())
}

pub fn get_playlist_tracks(cfg: &Config, id: &str) -> Result<Vec<PlaylistTrack>, NavidromeError> {
    let url = format!("{}/rest/getPlaylist?id={}&{}", cfg.navidrome_url, id, auth_params(cfg));
    let resp = make_client()?.get(url).send()?.error_for_status()?;
    let parsed: SubsonicResponse<PlaylistWrapper> = resp.json()?;

    if parsed.response.status != "ok" {
        return Err(NavidromeError::InvalidResponse);
    }

    Ok(parsed.response.playlist.entry.into_iter().map(|e| PlaylistTrack {
        id: e.id,
        title: e.title.unwrap_or_default(),
        artist: e.artist.unwrap_or_default(),
        duration: e.duration.unwrap_or(0),
        cover_art_id: e.cover_art,
    }).collect())
}

pub fn stream_url(cfg: &Config, track_id: &str) -> String {
    format!("{}/rest/stream?id={}&{}", cfg.navidrome_url, track_id, auth_params(cfg))
}

pub fn fetch_cover_art_bytes(cfg: &Config, cover_art_id: &str) -> Option<Vec<u8>> {
    let url = format!(
        "{}/rest/getCoverArt?id={}&size=120&{}",
        cfg.navidrome_url, cover_art_id, auth_params(cfg)
    );
    let bytes = make_client().ok()?
        .get(url)
        .send().ok()?
        .error_for_status().ok()?
        .bytes().ok()?;
    Some(bytes.to_vec())
}
