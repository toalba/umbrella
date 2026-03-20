# twa_render

Discord bot that renders World of Warships replay files into minimap timelapse videos.

## Architecture

- Single Rust binary using `poise` (Discord slash command framework built on serenity)
- Uses `wows-toolkit` crates as git dependencies:
  - `wows_replays` — parses `.wowsreplay` files (Blowfish decrypt, zlib decompress, BigWorld packet decode)
  - `wows_minimap_renderer` — CPU-based 2D minimap renderer (tiny-skia → H.264 → MP4)
  - `wowsunpack` — game data VFS, entity specs, GameParams

## Key files

- `src/main.rs` — Discord bot entry point, slash commands (`/replay`, `/ping`)
- `src/render.rs` — Render pipeline: loads game data once at startup, renders replays on demand

## Build & Run

```bash
# Requires Rust 1.92+
cp .env.example .env  # fill in DISCORD_TOKEN, WOWS_GAME_DIR, WOWS_GAME_VERSION
cargo build --release
./target/release/twa_render
```

## How it works

1. Game data (VFS, entity specs, GameParams, translations) loaded once at startup
2. User uploads `.wowsreplay` via `/replay` slash command
3. Bot downloads attachment, parses replay, runs BattleController to reconstruct game state
4. MinimapRenderer + VideoEncoder produce a 60s 30fps MP4 compressed timelapse
5. Bot uploads the MP4 back to Discord with a summary

## Dependencies

- `poise` 0.6 for Discord slash commands
- `tokio` for async runtime
- `tempfile` for render output
- CPU H.264 encoding via `openh264` (no GPU required)
