use std::borrow::Cow;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use tracing::{info, warn};
use wowsunpack::data::{DataFileWithCallback, Version};
use wowsunpack::game_data;
use wowsunpack::game_params::provider::GameMetadataProvider;
use wowsunpack::game_params::types::{GameParamProvider, Param};
use wowsunpack::rpc::entitydefs::parse_scripts;
use wowsunpack::vfs::VfsPath;
use wowsunpack::vfs::impls::physical::PhysicalFS;

use wows_replays::ReplayFile;
use wows_replays::analyzer::Analyzer;
use wows_replays::analyzer::battle_controller::BattleController;
use wows_replays::game_constants::GameConstants;

use wows_minimap_renderer::assets::*;
use wows_minimap_renderer::config::RendererConfig;
use wows_minimap_renderer::drawing::ImageTarget;
use wows_minimap_renderer::renderer::MinimapRenderer;
use wows_minimap_renderer::video::VideoEncoder;

/// All the heavy game data that gets loaded once at startup and reused for every render.
pub struct GameData {
    pub vfs: VfsPath,
    pub specs: Vec<wowsunpack::rpc::entitydefs::EntitySpec>,
    pub game_params: GameMetadataProvider,
    pub controller_game_params: GameMetadataProvider,
}

impl GameData {
    /// Load game data from a full WoWS installation directory.
    pub fn from_game_dir(game_dir: &Path, version: &Version) -> Result<Self> {
        info!(build = %version.build, "Loading game data from install");
        let resources = game_data::load_game_resources(game_dir, version)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let vfs = resources.vfs;
        let specs = resources.specs;

        let mut game_params = GameMetadataProvider::from_vfs(&vfs)
            .map_err(|e| anyhow::anyhow!("Failed to load GameParams: {e:?}"))?;
        let mut controller_game_params = GameMetadataProvider::from_vfs(&vfs)
            .map_err(|e| anyhow::anyhow!("Failed to load controller GameParams: {e:?}"))?;

        let mo_path = game_data::translations_path(game_dir, version.build);
        load_translations_from_path(&mo_path, &mut game_params, &mut controller_game_params);

        Ok(Self { vfs, specs, game_params, controller_game_params })
    }

    /// Load game data from a pre-extracted directory.
    ///
    /// Expected structure:
    /// ```text
    /// extracted/
    /// ├── metadata.toml          # [metadata]\n version = "15.1.0"\n build = 12345678
    /// ├── game_params.rkyv       # rkyv-serialized Vec<Param>
    /// ├── translations/en/LC_MESSAGES/global.mo
    /// └── vfs/                   # physical copy of game assets
    /// ```
    pub fn from_extracted_dir(extracted_dir: &Path) -> Result<Self> {
        info!("Loading from extracted directory: {}", extracted_dir.display());

        // Read metadata
        let meta_path = extracted_dir.join("metadata.toml");
        let meta_str = std::fs::read_to_string(&meta_path)
            .with_context(|| format!("Missing metadata.toml in {}", extracted_dir.display()))?;
        let meta_table: toml::Table = meta_str.parse()
            .context("Invalid metadata.toml")?;
        let build = meta_table.get("build")
            .and_then(|v| v.as_integer())
            .context("metadata.toml missing 'build' field")? as u32;
        info!(build, "Extracted data build");

        // VFS from physical directory
        let vfs_root = extracted_dir.join("vfs");
        if !vfs_root.exists() {
            bail!("VFS directory not found: {}", vfs_root.display());
        }
        let vfs = VfsPath::new(PhysicalFS::new(&vfs_root));

        // Entity specs from VFS
        info!("Loading entity specs");
        let specs = {
            let vfs_ref = &vfs;
            let loader = DataFileWithCallback::new(move |path: &str| {
                let mut data = Vec::new();
                vfs_ref.join(path)?.open_file()?.read_to_end(&mut data)?;
                Ok(Cow::Owned(data))
            });
            parse_scripts(&loader)
                .map_err(|e| anyhow::anyhow!("Failed to parse entity specs: {e:?}"))?
        };

        // GameParams from rkyv
        let rkyv_path = extracted_dir.join("game_params.rkyv");
        info!("Loading game params from rkyv");
        let rkyv_data = std::fs::read(&rkyv_path)
            .with_context(|| format!("Failed to read {}", rkyv_path.display()))?;
        let params: Vec<Param> = rkyv::from_bytes::<Vec<Param>, rkyv::rancor::Error>(&rkyv_data)
            .map_err(|e| anyhow::anyhow!("Failed to deserialize GameParams: {e}"))?;

        let mut game_params = GameMetadataProvider::from_params_no_specs(params.clone())
            .map_err(|e| anyhow::anyhow!("Failed to build GameMetadataProvider: {e:?}"))?;
        let mut controller_game_params = GameMetadataProvider::from_params_no_specs(params)
            .map_err(|e| anyhow::anyhow!("Failed to build controller GameMetadataProvider: {e:?}"))?;

        // Translations
        let mo_path = extracted_dir.join("translations/en/LC_MESSAGES/global.mo");
        load_translations_from_path(&mo_path, &mut game_params, &mut controller_game_params);

        Ok(Self { vfs, specs, game_params, controller_game_params })
    }
}

/// Render a replay file to an MP4 video.
pub fn render_replay(
    game_data: &GameData,
    replay_data: &[u8],
    output_path: &Path,
) -> Result<()> {
    let replay_file = replay_file_from_bytes(replay_data)
        .map_err(|e| anyhow::anyhow!("Failed to parse replay: {e:?}"))?;
    let replay_version = Version::from_client_exe(&replay_file.meta.clientVersionFromExe);

    let vfs = &game_data.vfs;

    info!("Loading fonts and icons");
    let game_fonts = load_game_fonts(vfs);
    let ship_icons = load_ship_icons(vfs);
    let plane_icons = load_plane_icons(vfs);
    let building_icons = load_building_icons(vfs);
    let consumable_icons = load_consumable_icons(vfs);
    let death_cause_icons = load_death_cause_icons(vfs, ICON_SIZE);
    let powerup_icons = load_powerup_icons(vfs, ICON_SIZE);
    let flag_icons = load_flag_icons(vfs);

    let game_constants = GameConstants::from_vfs(vfs);

    let map_name = &replay_file.meta.mapName;
    let map_image = load_map_image(map_name, vfs);
    let map_info = load_map_info(map_name, vfs);
    let game_duration = replay_file.meta.duration as f32;

    let config = RendererConfig::default();
    let mut options = config.into_render_options();
    options.show_stats_panel = true;

    let mut target = ImageTarget::with_stats_panel(
        map_image,
        game_fonts.clone(),
        ship_icons,
        plane_icons,
        building_icons,
        consumable_icons,
        death_cause_icons,
        powerup_icons,
        options.show_stats_panel,
    );

    // Load self-player ship silhouette
    let self_silhouette = replay_file.meta.vehicles.iter()
        .find(|v| v.relation == 0)
        .and_then(|v| {
            let param = GameParamProvider::game_param_by_id(&game_data.game_params, v.shipId)?;
            let path = format!("gui/ships_silhouettes/{}.png", param.index());
            let img = load_packed_image(&path, vfs)?;
            Some(img.into_rgba8())
        });

    let mut renderer = MinimapRenderer::new(map_info, &game_data.game_params, replay_version, options);
    renderer.set_fonts(game_fonts);
    renderer.set_flag_icons(flag_icons);
    if let Some(sil) = self_silhouette {
        renderer.set_self_silhouette(sil);
    }

    let (cw, ch) = target.canvas_size();
    let output_str = output_path.to_str().context("Invalid output path")?;
    let mut encoder = VideoEncoder::new(output_str, None, false, game_duration, cw, ch);
    encoder.set_prefer_cpu(true);
    encoder.init().map_err(|e| anyhow::anyhow!("Encoder init failed: {e:?}"))?;

    // Pre-scan for battle duration
    {
        let mut scan_parser = wows_replays::packet2::Parser::new(&game_data.specs);
        let mut scan_remaining = &replay_file.packet_data[..];
        let mut last_clock = wows_replays::types::GameClock(0.0);
        while !scan_remaining.is_empty() {
            match scan_parser.parse_packet(&mut scan_remaining) {
                Ok(packet) => {
                    last_clock = wows_replays::types::GameClock(packet.clock.0.max(last_clock.0));
                }
                Err(_) => break,
            }
        }
        if last_clock.seconds() > 0.0 {
            encoder.set_battle_duration(last_clock);
        }
    }

    let mut controller = BattleController::new(
        &replay_file.meta,
        &game_data.controller_game_params,
        Some(&game_constants),
    );

    let mut parser = wows_replays::packet2::Parser::new(&game_data.specs);
    let mut remaining = &replay_file.packet_data[..];
    let mut prev_clock = wows_replays::types::GameClock(0.0);

    while !remaining.is_empty() {
        let packet = parser.parse_packet(&mut remaining)
            .map_err(|e| anyhow::anyhow!("Packet parse error: {e:?}"))?;

        if packet.clock != prev_clock && prev_clock.seconds() > 0.0 {
            renderer.populate_players(&controller);
            renderer.update_squadron_info(&controller);
            renderer.update_ship_abilities(&controller);
            encoder.advance_clock(prev_clock, &controller, &mut renderer, &mut target);
            prev_clock = packet.clock;
        } else if prev_clock.seconds() == 0.0 {
            prev_clock = packet.clock;
        }

        controller.process(&packet);
    }

    // Final tick
    if prev_clock.seconds() > 0.0 {
        renderer.populate_players(&controller);
        renderer.update_squadron_info(&controller);
        renderer.update_ship_abilities(&controller);
        encoder.advance_clock(prev_clock, &controller, &mut renderer, &mut target);
    }

    controller.finish();
    encoder.finish(&controller, &mut renderer, &mut target)
        .map_err(|e| anyhow::anyhow!("Encoder finish failed: {e:?}"))?;

    info!("Render complete: {}", output_path.display());
    Ok(())
}

/// Build a short text summary from replay metadata.
pub fn replay_summary(replay_data: &[u8]) -> Result<String> {
    let replay_file = replay_file_from_bytes(replay_data)
        .map_err(|e| anyhow::anyhow!("Failed to parse replay: {e:?}"))?;
    let meta = &replay_file.meta;

    let player_name = &meta.playerName;
    let map_name = &meta.mapDisplayName;
    let vehicles_count = meta.vehicles.len();
    let duration_mins = meta.duration as f64 / 60.0;

    Ok(format!(
        "**{player_name}** on **{map_name}** ({vehicles_count} players, {duration_mins:.1} min)"
    ))
}

/// Write replay bytes to a temp file and parse via ReplayFile::from_file.
fn replay_file_from_bytes(data: &[u8]) -> Result<ReplayFile> {
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new().context("Failed to create temp file")?;
    tmp.write_all(data).context("Failed to write replay to temp file")?;
    tmp.flush()?;
    ReplayFile::from_file(tmp.path())
        .map_err(|e| anyhow::anyhow!("Failed to parse replay: {e:?}"))
}

fn load_translations_from_path(
    mo_path: &Path,
    game_params: &mut GameMetadataProvider,
    controller_game_params: &mut GameMetadataProvider,
) {
    if mo_path.exists() {
        if let Ok(file) = File::open(mo_path) {
            if let Ok(catalog) = gettext::Catalog::parse(file) {
                game_params.set_translations(catalog);
                if let Ok(file2) = File::open(mo_path) {
                    if let Ok(catalog2) = gettext::Catalog::parse(file2) {
                        controller_game_params.set_translations(catalog2);
                    }
                }
            }
        }
    } else {
        warn!(path = ?mo_path, "Translations not found");
    }
}
