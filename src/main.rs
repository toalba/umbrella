mod render;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use poise::serenity_prelude as serenity;
use tracing::info;

/// Shared state available to all bot commands.
pub struct BotState {
    game_data: render::GameData,
}

type PoiseError = Box<dyn std::error::Error + Send + Sync>;
type PoiseContext<'a> = poise::Context<'a, Arc<BotState>, PoiseError>;

/// Upload a `.wowsreplay` file and get a rendered minimap timelapse video.
#[poise::command(slash_command)]
async fn replay(
    ctx: PoiseContext<'_>,
    #[description = "The .wowsreplay file"] replay_file: serenity::Attachment,
) -> Result<(), PoiseError> {
    // Validate file
    if !replay_file.filename.ends_with(".wowsreplay") {
        ctx.say("Please upload a `.wowsreplay` file.").await?;
        return Ok(());
    }

    // Size limit: 50 MB
    if replay_file.size > 50 * 1024 * 1024 {
        ctx.say("Replay file too large (max 50 MB).").await?;
        return Ok(());
    }

    // Defer reply since rendering takes a while
    ctx.defer().await?;

    // Download the replay
    let replay_data = replay_file.download().await
        .map_err(|e| anyhow::anyhow!("Failed to download attachment: {e}"))?;

    // Get summary before rendering
    let summary = render::replay_summary(&replay_data).unwrap_or_else(|_| "Replay".to_string());

    // Render in a blocking task (CPU-intensive)
    let game_data = ctx.data();
    let game_data_ref = Arc::clone(game_data);

    let output_path = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
        let tmp_dir = tempfile::tempdir().context("Failed to create temp dir")?;
        let output = tmp_dir.path().join("replay.mp4");

        render::render_replay(&game_data_ref.game_data, &replay_data, &output)?;

        // Keep the tmpdir alive by leaking it — we'll clean up after upload
        let path = output.clone();
        // Persist the tempdir so it isn't deleted when this closure returns
        let _ = tmp_dir.keep();
        Ok(path)
    })
    .await
    .map_err(|e| anyhow::anyhow!("Render task panicked: {e}"))?
    .map_err(|e| anyhow::anyhow!("Render failed: {e}"))?;

    // Upload the video
    let video_data = tokio::fs::read(&output_path).await
        .context("Failed to read rendered video")?;

    let attachment = serenity::CreateAttachment::bytes(video_data, "minimap.mp4");
    let reply = poise::CreateReply::default()
        .content(summary)
        .attachment(attachment);

    ctx.send(reply).await?;

    // Clean up temp files
    if let Some(parent) = output_path.parent() {
        let _ = tokio::fs::remove_dir_all(parent).await;
    }

    Ok(())
}

/// Check if the bot is alive.
#[poise::command(slash_command)]
async fn ping(ctx: PoiseContext<'_>) -> Result<(), PoiseError> {
    ctx.say("Pong! Bot is running.").await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file if present
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let token = std::env::var("DISCORD_TOKEN")
        .context("DISCORD_TOKEN env var is required")?;

    // Support two modes: extracted data dir (for servers) or full game install (for local dev)
    let game_data = if let Ok(data_dir) = std::env::var("WOWS_DATA_DIR") {
        info!("Loading pre-extracted game data from: {data_dir}");
        render::GameData::from_extracted_dir(&PathBuf::from(&data_dir))
            .context("Failed to load extracted game data")?
    } else if let Ok(game_dir) = std::env::var("WOWS_GAME_DIR") {
        let game_version = std::env::var("WOWS_GAME_VERSION")
            .context("WOWS_GAME_VERSION is required when using WOWS_GAME_DIR")?;
        info!("Loading game data from install: {game_dir}");
        let version = wowsunpack::data::Version::from_client_exe(&game_version);
        render::GameData::from_game_dir(&PathBuf::from(&game_dir), &version)
            .context("Failed to load WoWS game data")?
    } else {
        anyhow::bail!(
            "Set either WOWS_DATA_DIR (path to extracted game data) \
             or WOWS_GAME_DIR + WOWS_GAME_VERSION (path to WoWS install)"
        );
    };

    let state = Arc::new(BotState { game_data });

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![replay(), ping()],
            ..Default::default()
        })
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                info!("Bot is ready! Commands registered globally.");
                Ok(state)
            })
        })
        .build();

    let intents = serenity::GatewayIntents::non_privileged();

    let mut client = serenity::ClientBuilder::new(&token, intents)
        .framework(framework)
        .await
        .context("Failed to create Discord client")?;

    info!("Starting bot...");
    client.start().await.context("Bot crashed")?;

    Ok(())
}
