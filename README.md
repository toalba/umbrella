# Umbrella

The ultimate protection against rain, never get wet again. Renders your replays.

Discord bot that renders World of Warships replay files into minimap timelapse videos. Upload a `.wowsreplay` via `/replay` and get back a 60-second MP4 timelapse.

## Setup

### 1. Extract game data

The bot needs pre-extracted WoWS game data. On your local machine with WoWS installed, build and run [wows-data-mgr](https://github.com/landaire/wows-toolkit) to extract it:

```bash
cargo build --release -p wows-data-mgr
./target/release/wows-data-mgr dump-renderer-data --extracted-dir ./extracted "C:\Games\World_of_Warships"
```

You also need to copy `scripts/entity_defs` from your WoWS installation into the extracted `vfs/`:

```bash
cp -r "C:\Games\World_of_Warships\res\scripts" ./extracted/vfs/scripts
```

Upload the extracted directory to your server.

### 2. Configure

```bash
cp .env.example .env
```

Fill in:
- `DISCORD_TOKEN` — your bot token from the [Discord Developer Portal](https://discord.com/developers/applications)
- `WOWS_DATA_DIR` — path to the extracted game data directory

### 3. Run with Docker

```bash
docker compose up -d
```

### 4. Run without Docker

Requires Rust 1.92+.

```bash
cargo build --release
./target/release/twa_render
```

## Updating game data

When a new WoWS version is released, re-extract the game data:

- **Linux/macOS:** `./update.sh`
- **Windows:** `.\update.ps1`

## Commands

| Command | Description |
|---------|-------------|
| `/replay` | Upload a `.wowsreplay` file and get a rendered minimap timelapse |
| `/ping` | Check if the bot is alive |

## Credits

This project is built on top of the excellent work by others:

- [wows-toolkit](https://github.com/landaire/wows-toolkit) by [@landaire](https://github.com/landaire) — replay parsing (`wows_replays`), minimap rendering (`wows_minimap_renderer`), and game data unpacking (`wowsunpack`). This project would not exist without it.
- [poise](https://github.com/serenity-rs/poise) — Discord slash command framework built on [serenity](https://github.com/serenity-rs/serenity)
- [openh264](https://github.com/cisco/openh264) by Cisco — CPU H.264 encoding
