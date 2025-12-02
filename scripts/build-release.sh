#!/usr/bin/env bash

set -e

echo " Building Sonix Lyrics (release)..."

# Clean old build
rm -rf dist
mkdir dist

# Optimized release build
cargo build --release

# Strip binary for smaller size
echo " Stripping binary..."
strip target/release/sonix_lyrics || true

# Move to dist folder
cp target/release/sonix_lyrics dist/
echo " Build complete!"
echo "➡ Output: dist/sonix_lyrics"
