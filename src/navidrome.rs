use reqwest::blocking::ClientBuilder;
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
    #[serde(default)]
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

// ---- helper: build base auth query string ----

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
    }).collect())
}

pub fn jukebox_play(cfg: &Config, track_ids: &[String]) -> Result<(), NavidromeError> {
    let id_params: String = track_ids.iter().map(|id| format!("&id={}", id)).collect();
    let url = format!(
        "{}/rest/jukeboxControl?action=set&{}{}",
        cfg.navidrome_url, auth_params(cfg), id_params
    );
    make_client()?.get(url).send()?.error_for_status()?;
    Ok(())
}

pub fn get_current_track(cfg: &Config) -> Result<Track, NavidromeError> {
    let url = format!("{}/rest/getNowPlaying?{}", cfg.navidrome_url, auth_params(cfg));
    let resp = make_client()?.get(url).send()?.error_for_status()?;
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
