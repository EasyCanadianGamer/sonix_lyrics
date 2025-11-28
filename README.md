# Sonix Lyrics

A fast terminal-based synced lyrics + karaoke client for Navidrome, built in Rust with Ratatui.

![Sonix Lyrics Screenshot](assets/screenshot.png)

---

## Features

- **Real-time synced lyrics (LRC)**
- **Karaoke mode** (word-by-word) ( a bit buggy )
- Auto-detects currently playing track from **Navidrome**
- Smooth, drift-free playback timer
- Clean TUI using Ratatui + Crossterm
- Fully configurable via `config.conf`
- Works without `.env` files
- Logging to `sonix_lyrics.log`

---

## Installation

### **Clone the repository**

```bash
git clone https://github.com/CanadianGamer23/sonix_lyrics
cd sonix_lyrics

# Run in debug mode
cargo run

# Build release version
./build-release.sh

# Output will appear in:
./dist/sonix_lyrics


###  Configuration

Create a file named:

`config.conf`

in the same directory as the binary.
Example:

```ini
# Sonix Lyrics Configuration

# Navidrome Server Info
NAVIDROME_URL = https://your.navidrome.server
NAVIDROME_USER = your_user
NAVIDROME_TOKEN = your_token
NAVIDROME_SALT = your_salt

# TUI Refresh Interval (seconds)
REFRESH_INTERVAL = 2

# Enable Karaoke Word Highlighting
KARAOKE_ENABLED = false
```

## Usage

Start the TUI:

```bash
./sonix_lyrics
```

Controls:


| Key    | Action             |
| -------- | -------------------- |
| q      | Quit               |
| r      | Refresh metadata   |
| j / ↓ | Scroll lyrics down |
| k / ↑ | Scroll lyrics up   |

---

##  Project Structure

```
src/
  ├─ main.rs      # TUI runtime
  ├─ navidrome.rs  # Navidrome API
  ├─ lyrics.rs     # Lyrics fetching + parsing
  ├─ config.rs     # Config loader
config.conf       # User configuration
build-release.sh  # Build script
LICENSE           # MIT license
README.md         # This file
```

---

##  License

This project is licensed under the MIT License — see LICENSE

---

##  Contributing

Pull requests and improvements are welcome.
Feel free to open issues or feature requests.

---

## Credit

Made by CanadianGamer
Powered by:

- Rust
- Ratatui
- Crossterm
- Navidrome
- LRC sources from lrclib.net
