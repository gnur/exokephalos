use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use xo_core::iroh_node::IrohNode;
use xo_core::records::WorkspaceRecords;
use xo_core::{ActorId, CURRENT_SCHEMA, HlcClock, NoteRevision};

#[derive(Debug, Parser)]
#[command(name = "xo-admin", version, about = "Workspace administration")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate every Markdown file in an existing workspace without modifying it.
    AuditWorkspace { path: PathBuf },
    /// Import a Markdown workspace into a new native replicated workspace.
    ImportWorkspace {
        /// Existing workspace to read. This directory is never modified.
        source: PathBuf,
        /// New persistent Iroh state directory, which must be outside the source.
        state_dir: PathBuf,
    },
}

#[tokio::main]
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
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ImportResult {
    workspace_id: String,
    ticket: String,
    imported: usize,
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
    let report = xo_core::projection::scan_for_import(&source)?;
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
    let result = ImportResult {
        workspace_id: workspace.id().to_string(),
        ticket: workspace.share(true).await?,
        imported: report.notes.len(),
    };
    node.shutdown().await?;
    Ok(result)
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
            if entry.file_name() != ".exo" {
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
    use xo_core::domain::{Frontmatter, FrontmatterValue};
    use xo_core::{Note, NoteId};

    use super::*;

    #[tokio::test]
    async fn import_does_not_modify_the_source_workspace() -> Result<()> {
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
                    FrontmatterValue::String("Legacy".to_owned()),
                ),
            ]),
            body: "unchanged\n".to_owned(),
            path: "notes/legacy.md".to_owned(),
        };
        xo_core::projection::materialize(&source, &note)?;
        let before = std::fs::read(source.join(&note.path))?;

        let imported = import_workspace(&source, &directory.path().join("native-state")).await?;
        assert_eq!(imported.imported, 1);
        assert_eq!(std::fs::read(source.join(&note.path))?, before);
        assert!(!source.join(".exo").exists());
        Ok(())
    }

    #[tokio::test]
    async fn import_rejects_state_inside_source() -> Result<()> {
        let directory = tempfile::tempdir()?;
        assert!(
            import_workspace(directory.path(), &directory.path().join(".exo/native"))
                .await
                .is_err()
        );
        assert!(!directory.path().join(".exo").exists());
        Ok(())
    }
}
