use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use xo_core::iroh_node::{IrohNode, validate_writable_ticket};
use xo_core::projection::{ProjectedAsset, ProjectionState};
use xo_core::records::WorkspaceRecords;
use xo_core::{ActorId, AssetId, CURRENT_SCHEMA, HlcClock, Note, NoteRevision};

#[derive(Debug, Parser)]
#[command(
    name = "xo-admin",
    version = xo_core::version::VERSION,
    about = "Workspace administration"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate every Markdown file in an existing workspace without modifying it.
    AuditWorkspace { path: PathBuf },
    /// Import Markdown, assets, and Steel configuration into a replicated workspace.
    ImportWorkspace {
        /// Existing workspace to read. This directory is never modified.
        source: PathBuf,
        /// New persistent Iroh state directory, which must be outside the source.
        state_dir: PathBuf,
    },
    /// Import a writable workspace ticket into an offline peer state directory.
    ImportTicket { state_dir: PathBuf, ticket: String },
    /// Create and verify an offline backup of a stopped peer state directory.
    Backup {
        state_dir: PathBuf,
        destination: PathBuf,
    },
    /// Verify every file in a backup against its manifest.
    VerifyBackup { backup: PathBuf },
    /// Restore a verified backup into a new or empty state directory.
    Restore { backup: PathBuf, state_dir: PathBuf },
    /// Create a workspace invitation from an offline peer state directory.
    Invite {
        state_dir: PathBuf,
        workspace: String,
        #[arg(long)]
        read_only: bool,
    },
    /// List replicated workspace devices.
    DeviceList {
        state_dir: PathBuf,
        workspace: String,
    },
    /// Retire a device in replicated workspace metadata.
    RetireDevice {
        state_dir: PathBuf,
        workspace: String,
        endpoint: String,
    },
    /// Checkpoint accepted state into a fresh namespace and print reinvitations.
    RotateNamespace {
        state_dir: PathBuf,
        workspace: String,
    },
    /// Print record and projection diagnostics for a workspace.
    Diagnostics {
        state_dir: PathBuf,
        workspace: String,
    },
    /// Update replicated relay mode and bootstrap peers.
    SetRelay {
        state_dir: PathBuf,
        workspace: String,
        mode: String,
        #[arg(long = "bootstrap-peer")]
        bootstrap_peers: Vec<String>,
    },
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::AuditWorkspace { path } => {
            let mut valid = 0_u64;
            let mut invalid = 0_u64;
            audit(&path, &mut valid, &mut invalid)?;
            println!("valid={valid} invalid={invalid}");
            if invalid > 0 {
                std::process::exit(1);
            }
        }
        Command::ImportWorkspace { source, state_dir } => {
            let imported = import_workspace(&source, &state_dir).await?;
            println!("workspace_id={}", imported.workspace_id);
            println!("ticket={}", imported.ticket);
            println!("imported={}", imported.imported);
            println!("assets={}", imported.assets);
            println!("configs={}", imported.configs);
        }
        Command::ImportTicket { state_dir, ticket } => {
            let imported = import_ticket(&state_dir, &ticket).await?;
            println!("workspace_id={}", imported.workspace_id);
            println!("ticket={}", imported.ticket);
        }
        Command::Backup {
            state_dir,
            destination,
        } => {
            let manifest = xo_core::backup::create_backup(state_dir, destination)?;
            println!("files={}", manifest.entries.len());
        }
        Command::VerifyBackup { backup } => {
            let manifest = xo_core::backup::verify_backup(backup)?;
            println!("valid=true files={}", manifest.entries.len());
        }
        Command::Restore { backup, state_dir } => {
            let manifest = xo_core::backup::restore_backup(backup, state_dir)?;
            println!("restored={}", manifest.entries.len());
        }
        Command::Invite {
            state_dir,
            workspace,
            read_only,
        } => {
            let node = IrohNode::persistent(state_dir).await?;
            let workspace = node
                .open_workspace_str(&workspace)
                .await?
                .context("workspace is not present in this peer")?;
            println!("ticket={}", workspace.share(!read_only).await?);
            node.shutdown().await?;
        }
        Command::DeviceList {
            state_dir,
            workspace,
        } => {
            let node = IrohNode::persistent(state_dir).await?;
            let workspace = node
                .open_workspace_str(&workspace)
                .await?
                .context("workspace is not present in this peer")?;
            for device in WorkspaceRecords::new(&workspace).list_devices().await? {
                println!("{}", serde_json::to_string(&device)?);
            }
            node.shutdown().await?;
        }
        Command::RetireDevice {
            state_dir,
            workspace,
            endpoint,
        } => {
            retire_device(&state_dir, &workspace, &endpoint).await?;
        }
        Command::RotateNamespace {
            state_dir,
            workspace,
        } => {
            rotate_namespace(&state_dir, &workspace).await?;
        }
        Command::Diagnostics {
            state_dir,
            workspace,
        } => {
            print_diagnostics(&state_dir, &workspace).await?;
        }
        Command::SetRelay {
            state_dir,
            workspace,
            mode,
            bootstrap_peers,
        } => {
            set_relay(&state_dir, &workspace, mode, bootstrap_peers).await?;
        }
    }
    Ok(())
}

async fn print_diagnostics(state_dir: &Path, workspace_id: &str) -> Result<()> {
    let node = IrohNode::persistent(state_dir).await?;
    let workspace = node
        .open_workspace_str(workspace_id)
        .await?
        .context("workspace is not present in this peer")?;
    let snapshot = WorkspaceRecords::new(&workspace).snapshot().await?;
    println!(
        "notes={} assets={} configs={} devices={} diagnostics={}",
        snapshot.notes.len(),
        snapshot.assets.len(),
        snapshot.configs.len(),
        snapshot.devices.len(),
        snapshot.diagnostics.len()
    );
    for diagnostic in snapshot.diagnostics {
        eprintln!(
            "{} [{}]: {}",
            diagnostic.path, diagnostic.code, diagnostic.message
        );
    }
    node.shutdown().await?;
    Ok(())
}

async fn set_relay(
    state_dir: &Path,
    workspace_id: &str,
    mode: String,
    bootstrap_peers: Vec<String>,
) -> Result<()> {
    let node = IrohNode::persistent(state_dir).await?;
    let workspace = node
        .open_workspace_str(workspace_id)
        .await?
        .context("workspace is not present in this peer")?;
    let records = WorkspaceRecords::new(&workspace);
    let mut descriptor = records
        .descriptor()
        .await?
        .context("workspace has no replicated descriptor")?;
    descriptor.relay_mode = mode;
    descriptor.bootstrap_peers = bootstrap_peers;
    records.put_descriptor(&descriptor).await?;
    println!("relay_mode={}", descriptor.relay_mode);
    node.shutdown().await?;
    Ok(())
}

async fn retire_device(state_dir: &Path, workspace_id: &str, endpoint: &str) -> Result<()> {
    let node = IrohNode::persistent(state_dir).await?;
    let workspace = node
        .open_workspace_str(workspace_id)
        .await?
        .context("workspace is not present in this peer")?;
    let records = WorkspaceRecords::new(&workspace);
    let mut device = records
        .list_devices()
        .await?
        .into_iter()
        .find(|device| device.endpoint_id == endpoint)
        .context("device is not present in this workspace")?;
    let wall_clock_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_millis()
        .try_into()
        .context("system time does not fit in an HLC timestamp")?;
    device.retired_at = Some(HlcClock::new(records.actor_id()).next(wall_clock_ms));
    records.put_device(&device).await?;
    println!("retired={endpoint}");
    node.shutdown().await?;
    Ok(())
}

async fn rotate_namespace(state_dir: &Path, workspace_id: &str) -> Result<()> {
    let node = IrohNode::persistent(state_dir).await?;
    let workspace = node
        .open_workspace_str(workspace_id)
        .await?
        .context("workspace is not present in this peer")?;
    let wall_clock_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_millis()
        .try_into()
        .context("system time does not fit in an HLC timestamp")?;
    let rotation = xo_core::rotation::rotate_workspace(&node, &workspace, wall_clock_ms).await?;
    println!("archived_workspace_id={}", rotation.archived_workspace_id);
    println!("workspace_id={}", rotation.workspace_id);
    println!("ticket={}", rotation.writable_ticket);
    println!("copied_notes={}", rotation.copied_notes);
    println!("copied_assets={}", rotation.copied_assets);
    println!("copied_configs={}", rotation.copied_configs);
    for endpoint in rotation.reinvite_endpoints {
        println!("reinvite_endpoint={endpoint}");
    }
    node.shutdown().await?;
    Ok(())
}

#[derive(Debug)]
struct ImportResult {
    workspace_id: String,
    ticket: String,
    imported: usize,
    assets: usize,
    configs: usize,
}

#[derive(Debug)]
struct TicketImportResult {
    workspace_id: String,
    ticket: String,
}

async fn import_ticket(state_dir: &Path, ticket: &str) -> Result<TicketImportResult> {
    validate_writable_ticket(ticket)?;
    let node = IrohNode::persistent(state_dir).await?;
    let workspace = node.import_writable_workspace_synced(ticket).await?;
    let result = TicketImportResult {
        workspace_id: workspace.id().to_string(),
        ticket: workspace.share(true).await?,
    };
    node.shutdown().await?;
    Ok(result)
}

async fn import_workspace(source: &Path, state_dir: &Path) -> Result<ImportResult> {
    let source = source
        .canonicalize()
        .with_context(|| format!("resolve source workspace {}", source.display()))?;
    let state_dir = resolved_target(state_dir)?;
    if state_dir.starts_with(&source) {
        bail!(
            "state directory {} must be outside source workspace {}",
            state_dir.display(),
            source.display()
        );
    }

    // Finish all validation before creating native state so a rejected import has no side effects.
    let mut report = xo_core::projection::scan_for_import(&source)?;
    if !report.diagnostics.is_empty() {
        let details = report
            .diagnostics
            .iter()
            .take(10)
            .map(|diagnostic| {
                format!(
                    "{} [{}]: {}",
                    diagnostic.path, diagnostic.code, diagnostic.message
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "source workspace has {} diagnostic(s); import aborted\n{details}",
            report.diagnostics.len()
        );
    }
    for note in &mut report.notes {
        xo_core::timestamp::localize_utc_frontmatter(&mut note.frontmatter)
            .context("determine system time zone for imported timestamps")?;
    }
    let source_assets = scan_assets(&source)?;
    let source_configs = scan_configs(&source)?;

    let node = IrohNode::persistent(&state_dir).await?;
    let workspace = node.create_workspace().await?;
    let actor = ActorId::new(workspace.author_id().to_string());
    let mut clock = HlcClock::new(actor.clone());
    let wall_clock_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_millis()
        .try_into()
        .context("system time does not fit in an HLC timestamp")?;
    let records = WorkspaceRecords::new(&workspace);
    for note in &report.notes {
        records
            .commit_revision(&NoteRevision {
                schema: CURRENT_SCHEMA,
                note_id: note.id.clone(),
                frontmatter: note.frontmatter.clone(),
                body: note.body.clone(),
                materialized_path: note.path.clone(),
                hlc: clock.next(wall_clock_ms),
                author_id: actor.clone(),
                predecessors: BTreeSet::new(),
                deleted: false,
            })
            .await?;
    }
    let mut imported_assets = Vec::new();
    for asset in source_assets {
        let record = records
            .put_asset(asset.id, asset.mime, asset.path, asset.bytes)
            .await?;
        imported_assets.push(
            records
                .get_asset(&record.id)
                .await?
                .context("imported asset record disappeared")?,
        );
    }
    for config in &source_configs {
        records
            .put_config(
                &config.path,
                config.bytes.clone(),
                clock.next(wall_clock_ms),
                BTreeSet::new(),
            )
            .await?;
    }
    verify_roundtrip(&records, &report.notes, &imported_assets, &source_configs).await?;
    let result = ImportResult {
        workspace_id: workspace.id().to_string(),
        ticket: workspace.share(true).await?,
        imported: report.notes.len(),
        assets: imported_assets.len(),
        configs: source_configs.len(),
    };
    node.shutdown().await?;
    Ok(result)
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct SourceConfig {
    path: String,
    bytes: Vec<u8>,
}

fn scan_configs(source: &Path) -> Result<Vec<SourceConfig>> {
    let mut paths = Vec::new();
    for relative in ["xo.scm", "modules", "plugins"] {
        let path = source.join(relative);
        if path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!(
                "configuration symlinks are not imported: {}",
                path.display()
            );
        }
        if path.is_file() {
            paths.push(path);
        } else if path.is_dir() {
            collect_config_files(source, &path, &mut paths)?;
        }
    }
    paths.sort();
    let configs = paths
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(source)
                .context("configuration is outside source workspace")?
                .to_string_lossy()
                .replace('\\', "/");
            if !xo_core::steel_runtime::valid_config_path(&relative) {
                bail!("invalid workspace configuration path {relative}");
            }
            Ok(SourceConfig {
                path: relative,
                bytes: std::fs::read(path)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if !configs.is_empty() {
        let main = configs
            .iter()
            .find(|config| config.path == "xo.scm")
            .context("modules and plugins require a root xo.scm configuration")?;
        let source = String::from_utf8(main.bytes.clone()).context("xo.scm is not UTF-8")?;
        let modules = configs
            .iter()
            .filter(|config| config.path != "xo.scm")
            .map(|config| {
                Ok((
                    config.path.clone(),
                    String::from_utf8(config.bytes.clone())
                        .with_context(|| format!("{} is not UTF-8", config.path))?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        xo_core::steel_runtime::SteelWorkspace::load(
            &source,
            &modules,
            "1970-01-01T00:00:00+00:00",
        )
        .context("validate imported workspace configuration")?;
    }
    Ok(configs)
}

fn collect_config_files(source: &Path, directory: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            bail!(
                "configuration symlinks are not imported: {}",
                path.display()
            );
        }
        if file_type.is_dir() {
            collect_config_files(source, &path, paths)?;
        } else if file_type.is_file() && path.extension().is_some_and(|value| value == "scm") {
            let relative = path.strip_prefix(source).unwrap_or(&path);
            if !relative.components().any(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .is_some_and(|name| name.starts_with('.'))
            }) {
                paths.push(path);
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
struct SourceAsset {
    id: AssetId,
    path: String,
    mime: String,
    bytes: Vec<u8>,
}

fn scan_assets(source: &Path) -> Result<Vec<SourceAsset>> {
    let assets_root = source.join("assets");
    if !assets_root.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    collect_assets(source, &assets_root, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(source)
                .context("asset is outside source workspace")?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = std::fs::read(&path)?;
            let mut identity = relative.as_bytes().to_vec();
            identity.push(0);
            identity.extend_from_slice(&bytes);
            Ok(SourceAsset {
                id: AssetId::new(blake3::hash(&identity).to_hex().to_string()),
                mime: asset_mime(&path).to_owned(),
                path: relative,
                bytes,
            })
        })
        .collect()
}

fn collect_assets(source: &Path, directory: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            bail!("asset symlinks are not imported: {}", path.display());
        }
        if file_type.is_dir() {
            let relative = path.strip_prefix(source).unwrap_or(&path);
            if relative.components().any(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .is_some_and(|name| name.starts_with('.'))
            }) {
                continue;
            }
            collect_assets(source, &path, paths)?;
        } else if file_type.is_file() {
            paths.push(path);
        }
    }
    Ok(())
}

fn asset_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        Some("txt" | "md") => "text/plain",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

async fn verify_roundtrip(
    records: &WorkspaceRecords<'_>,
    expected_notes: &[Note],
    expected_assets: &[ProjectedAsset],
    expected_configs: &[SourceConfig],
) -> Result<()> {
    let snapshot = records.snapshot().await?;
    if !snapshot.diagnostics.is_empty() {
        bail!(
            "authoritative import verification produced {} diagnostic(s)",
            snapshot.diagnostics.len()
        );
    }
    let clean = tempfile::tempdir()?;
    let projection = ProjectionState::open(clean.path())?;
    let note_report = projection.reconcile(&snapshot.notes)?;
    let asset_report = projection.reconcile_assets(&snapshot.assets)?;
    let config_report = projection.reconcile_configs(&snapshot.configs)?;
    if !note_report.diagnostics.is_empty()
        || !asset_report.diagnostics.is_empty()
        || !config_report.diagnostics.is_empty()
    {
        bail!("clean projection verification produced diagnostics");
    }
    let mut projected = xo_core::projection::scan(projection.root())?;
    let mut expected_notes = expected_notes.to_vec();
    projected
        .notes
        .sort_by(|left, right| left.id.cmp(&right.id));
    expected_notes.sort_by(|left, right| left.id.cmp(&right.id));
    if projected.notes != expected_notes || !projected.diagnostics.is_empty() {
        bail!("clean Markdown projection differs from the imported source");
    }
    if snapshot.assets != expected_assets {
        bail!("authoritative assets differ from the imported source");
    }
    for asset in expected_assets {
        let bytes = std::fs::read(projection.root().join(&asset.record.materialized_path))?;
        if bytes != asset.bytes {
            bail!(
                "projected asset differs: {}",
                asset.record.materialized_path
            );
        }
    }
    if snapshot.configs.len() != expected_configs.len() {
        bail!("authoritative configurations differ from imported source");
    }
    for config in expected_configs {
        if std::fs::read(projection.root().join(&config.path))? != config.bytes {
            bail!("projected configuration differs: {}", config.path);
        }
    }
    Ok(())
}

fn normalized_absolute(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    Ok(normalized)
}

fn resolved_target(path: &Path) -> Result<PathBuf> {
    let mut existing = normalized_absolute(path)?;
    let mut missing = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .context("state directory has no existing ancestor")?
            .to_os_string();
        missing.push(name);
        existing.pop();
    }
    let mut resolved = existing.canonicalize()?;
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn audit(path: &std::path::Path, valid: &mut u64, invalid: &mut u64) -> std::io::Result<()> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if entry.file_name() != ".xo" {
                audit(&path, valid, invalid)?;
            }
        } else if path.extension().is_some_and(|extension| extension == "md") {
            match std::fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|content| {
                    xo_core::markdown::parse(&content)
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                }) {
                Ok(()) => *valid += 1,
                Err(error) => {
                    *invalid += 1;
                    eprintln!("{}: {error}", path.display());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use xo_core::domain::{Frontmatter, FrontmatterValue};
    use xo_core::{Note, NoteId};

    use super::*;

    static IROH_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn import_does_not_modify_the_source_workspace() -> Result<()> {
        let _guard = IROH_TEST_LOCK.lock().await;
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("source");
        std::fs::create_dir(&source)?;
        let note = Note {
            id: NoteId::new("note002"),
            frontmatter: Frontmatter::from([
                (
                    "id".to_owned(),
                    FrontmatterValue::String("note002".to_owned()),
                ),
                (
                    "title".to_owned(),
                    FrontmatterValue::String("Current note".to_owned()),
                ),
                (
                    "created".to_owned(),
                    FrontmatterValue::String("2026-01-02T03:04:05Z".to_owned()),
                ),
            ]),
            body: "unchanged\n".to_owned(),
            path: "notes/current.md".to_owned(),
        };
        let source_path = source.join(&note.path);
        std::fs::create_dir_all(source_path.parent().context("note parent")?)?;
        std::fs::write(
            &source_path,
            xo_core::markdown::render(&note.frontmatter, &note.body)?,
        )?;
        let before = std::fs::read(source.join(&note.path))?;
        let asset_path = source.join("assets/images/cover.png");
        std::fs::create_dir_all(asset_path.parent().context("asset parent")?)?;
        std::fs::write(&asset_path, b"asset bytes")?;
        let asset_before = std::fs::read(&asset_path)?;
        let mut behavior = xo_core::behavior::WorkspaceBehavior::default();
        behavior.default_view = "notes".into();
        behavior.views = xo_core::behavior::default_views();
        behavior.views[0]
            .subviews
            .push(xo_core::behavior::SubviewDescriptor {
                id: "important".into(),
                name: "Important".into(),
                predicate: xo_core::behavior::Predicate::HasTag {
                    tag: "important".into(),
                },
            });
        let config = xo_core::steel_runtime::encode_config(&behavior, false);
        std::fs::write(source.join("xo.scm"), &config)?;

        let imported = import_workspace(&source, &directory.path().join("native-state")).await?;
        assert_eq!(imported.imported, 1);
        assert_eq!(imported.assets, 1);
        assert_eq!(imported.configs, 1);
        assert_eq!(std::fs::read_to_string(source.join("xo.scm"))?, config);
        assert_eq!(std::fs::read(source.join(&note.path))?, before);
        assert_eq!(std::fs::read(asset_path)?, asset_before);
        assert!(!source.join(".xo").exists());
        Ok(())
    }

    #[tokio::test]
    async fn import_rejects_state_inside_source() -> Result<()> {
        let _guard = IROH_TEST_LOCK.lock().await;
        let directory = tempfile::tempdir()?;
        assert!(
            import_workspace(directory.path(), &directory.path().join(".xo/native"))
                .await
                .is_err()
        );
        assert!(!directory.path().join(".xo").exists());
        Ok(())
    }

    #[tokio::test]
    async fn ticket_import_is_idempotent_and_reconnects_after_restart() -> Result<()> {
        let _guard = IROH_TEST_LOCK.lock().await;
        let directory = tempfile::tempdir()?;
        let source = IrohNode::persistent(directory.path().join("source")).await?;
        let workspace = source.create_workspace().await?;
        let workspace_id = workspace.id().to_string();
        let source_ticket = workspace.share(true).await?;
        let read_only_ticket = workspace.share(false).await?;
        let target_state = directory.path().join("target");

        let read_only_state = directory.path().join("read-only-target");
        assert!(
            import_ticket(&read_only_state, &read_only_ticket)
                .await
                .is_err()
        );
        assert!(!read_only_state.exists());
        let read_only_target = IrohNode::persistent(&read_only_state).await?;
        assert!(read_only_target.workspace_ids().await?.is_empty());
        read_only_target.shutdown().await?;

        let first = import_ticket(&target_state, &source_ticket).await?;
        let repeated = import_ticket(&target_state, &source_ticket).await?;
        assert_eq!(first.workspace_id, workspace_id);
        assert_eq!(repeated.workspace_id, workspace_id);
        assert_ne!(first.ticket, source_ticket);

        let target = IrohNode::persistent(&target_state).await?;
        let imported = target
            .open_workspace_str(&workspace_id)
            .await?
            .context("imported workspace missing after restart")?;
        imported.start_sync(&source_ticket).await?;
        workspace
            .put("note/import-ticket/revision/one", "after import")
            .await?;
        let mut replicated = None;
        for _ in 0..400 {
            match imported.get("note/import-ticket/revision/one").await {
                Ok(Some(value)) => {
                    replicated = Some(value);
                    break;
                }
                Ok(None) | Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        assert_eq!(replicated.as_deref(), Some(b"after import".as_slice()));

        target.shutdown().await?;
        source.shutdown().await?;
        Ok(())
    }
}
