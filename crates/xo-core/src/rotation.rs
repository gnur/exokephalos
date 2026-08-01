//! Hard revocation by checkpointing accepted state into a fresh Docs namespace.

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::iroh_node::{IrohNode, IrohWorkspace};
use crate::records::{RecordError, WorkspaceRecords, WorkspaceSnapshot};
use crate::{ActorId, HlcClock, NoteRevision, WorkspaceDescriptor, WorkspaceId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RotationResult {
    pub archived_workspace_id: String,
    pub workspace_id: String,
    pub writable_ticket: String,
    pub reinvite_endpoints: Vec<String>,
    pub copied_notes: usize,
    pub copied_assets: usize,
    pub copied_configs: usize,
}

/// Create a new namespace containing a checkpoint of all accepted visible state.
///
/// The returned write ticket must be distributed out of band only to the returned non-retired
/// endpoints. The source namespace remains available as a historical archive, but no rotation
/// capability or ticket is written into it.
pub async fn rotate_workspace(
    node: &IrohNode,
    source: &IrohWorkspace,
    wall_clock_ms: u64,
) -> Result<RotationResult> {
    let source_records = WorkspaceRecords::new(source);
    let snapshot = readable_snapshot(source_records).await?;
    if !snapshot.diagnostics.is_empty() {
        bail!(
            "rotation aborted because the source has {} diagnostic(s)",
            snapshot.diagnostics.len()
        );
    }
    let retired = snapshot
        .devices
        .iter()
        .filter(|device| device.retired_at.is_some())
        .count();
    if retired == 0 {
        bail!("rotation requires at least one retired device");
    }
    if snapshot
        .devices
        .iter()
        .any(|device| device.author_id == source_records.actor_id() && device.retired_at.is_some())
    {
        bail!("the rotating author is retired");
    }

    let target = node.create_workspace().await?;
    let target_records = WorkspaceRecords::new(&target);
    let actor = ActorId::new(target.author_id().to_string());
    let mut clock = HlcClock::new(actor.clone());
    let mut logical_time = wall_clock_ms;
    let mut copied_notes = 0_usize;
    for resolved in snapshot.resolved {
        let Some(visible) = resolved.visible else {
            continue;
        };
        target_records
            .commit_revision(&NoteRevision {
                schema: visible.schema,
                note_id: visible.note_id,
                frontmatter: visible.frontmatter,
                body: visible.body,
                materialized_path: visible.materialized_path,
                hlc: clock.next(logical_time),
                author_id: actor.clone(),
                predecessors: BTreeSet::new(),
                deleted: false,
            })
            .await?;
        copied_notes += 1;
        logical_time = logical_time.saturating_add(1);
    }

    let copied_assets = snapshot.assets.len();
    for asset in snapshot.assets {
        target_records
            .put_asset(
                asset.record.id,
                asset.record.mime,
                asset.record.materialized_path,
                asset.bytes,
            )
            .await?;
    }

    let copied_configs = snapshot.configs.len();
    for config in snapshot.configs {
        target_records
            .put_config(
                config.record.path,
                config.bytes,
                clock.next(logical_time),
                BTreeSet::new(),
            )
            .await?;
        logical_time = logical_time.saturating_add(1);
    }

    let source_descriptor = snapshot.descriptor;
    let read_ticket = target.share(false).await?;
    target_records
        .put_descriptor(&rotated_descriptor(&target, source_descriptor, read_ticket))
        .await?;

    let mut reinvite_endpoints = snapshot
        .devices
        .into_iter()
        .filter(|device| device.retired_at.is_none())
        .map(|device| device.endpoint_id)
        .collect::<Vec<_>>();
    reinvite_endpoints.sort();
    reinvite_endpoints.dedup();
    let writable_ticket = target
        .share(true)
        .await
        .context("create rotated workspace invitation")?;
    Ok(RotationResult {
        archived_workspace_id: source.id().to_string(),
        workspace_id: target.id().to_string(),
        writable_ticket,
        reinvite_endpoints,
        copied_notes,
        copied_assets,
        copied_configs,
    })
}

fn rotated_descriptor(
    target: &IrohWorkspace,
    source: Option<WorkspaceDescriptor>,
    read_ticket: String,
) -> WorkspaceDescriptor {
    WorkspaceDescriptor {
        schema: crate::CURRENT_SCHEMA,
        workspace_id: WorkspaceId::new(target.id().to_string()),
        docs_ticket: read_ticket,
        bootstrap_peers: source
            .as_ref()
            .map_or_else(Vec::new, |descriptor| descriptor.bootstrap_peers.clone()),
        relay_mode: source.as_ref().map_or_else(
            || "default".to_owned(),
            |descriptor| descriptor.relay_mode.clone(),
        ),
        encrypted_workspace_key: source.and_then(|descriptor| descriptor.encrypted_workspace_key),
        read_only: true,
    }
}

async fn readable_snapshot(records: WorkspaceRecords<'_>) -> Result<WorkspaceSnapshot> {
    let mut last_transport_error = None;
    for _ in 0..100 {
        match records.snapshot().await {
            Ok(snapshot) => return Ok(snapshot),
            Err(RecordError::Transport(error)) => {
                last_transport_error = Some(error);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(last_transport_error.context("source snapshot remained unavailable during rotation")?)
}

#[cfg(test)]
mod tests {
    use crate::domain::{DeviceRecord, Frontmatter, FrontmatterValue};
    use crate::records::{RecordError, WorkspaceRecords};
    use crate::{AssetId, CURRENT_SCHEMA, Hlc, NoteId};

    use super::*;

    async fn wait_for_devices(
        records: WorkspaceRecords<'_>,
        expected: usize,
    ) -> Result<Vec<DeviceRecord>> {
        for _ in 0..200 {
            match records.list_devices().await {
                Ok(devices) if devices.len() == expected => return Ok(devices),
                Ok(_) | Err(RecordError::Transport(_)) => {}
                Err(error) => return Err(error.into()),
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        bail!("devices did not converge")
    }

    async fn wait_for_checkpoint(records: WorkspaceRecords<'_>) -> Result<()> {
        for _ in 0..200 {
            match records.snapshot().await {
                Ok(snapshot)
                    if snapshot.notes.len() == 1
                        && snapshot.assets.len() == 1
                        && snapshot.configs.len() == 1 =>
                {
                    return Ok(());
                }
                Ok(_) | Err(RecordError::Transport(_)) => {}
                Err(error) => return Err(error.into()),
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        bail!("rotated checkpoint did not replicate")
    }

    async fn seed_source(
        active: &IrohNode,
        active_source: &IrohWorkspace,
        retired: &IrohNode,
        retired_source: &IrohWorkspace,
        source: &IrohWorkspace,
    ) -> Result<(Vec<DeviceRecord>, ActorId)> {
        let active_records = WorkspaceRecords::new(active_source);
        active_records
            .put_device(&device(active, active_records.actor_id(), "active"))
            .await
            .context("register active device")?;
        let retired_records = WorkspaceRecords::new(retired_source);
        retired_records
            .put_device(&device(retired, retired_records.actor_id(), "retired"))
            .await
            .context("register retired device")?;

        let source_records = WorkspaceRecords::new(source);
        let owner_actor = source_records.actor_id();
        let devices = wait_for_devices(source_records, 2).await?;
        source_records
            .commit_revision(&NoteRevision {
                schema: CURRENT_SCHEMA,
                note_id: NoteId::new("note002"),
                frontmatter: Frontmatter::from([(
                    "title".to_owned(),
                    FrontmatterValue::String("Checkpoint".to_owned()),
                )]),
                body: "accepted state".to_owned(),
                materialized_path: "notes/checkpoint.md".to_owned(),
                hlc: Hlc {
                    physical_ms: 100,
                    logical: 0,
                    actor_id: owner_actor.clone(),
                },
                author_id: owner_actor.clone(),
                predecessors: BTreeSet::new(),
                deleted: false,
            })
            .await
            .context("store source note")?;
        source_records
            .put_asset(
                AssetId::new("image001"),
                "image/png",
                "assets/checkpoint.png",
                b"asset checkpoint".to_vec(),
            )
            .await
            .context("store source asset")?;
        source_records
            .put_config(
                "xo.scm",
                b"(checkpoint)".to_vec(),
                Hlc {
                    physical_ms: 101,
                    logical: 0,
                    actor_id: owner_actor.clone(),
                },
                BTreeSet::new(),
            )
            .await
            .context("store source config")?;
        Ok((devices, owner_actor))
    }

    #[tokio::test]
    async fn rotation_reinvites_active_peer_and_excludes_retired_peer() -> Result<()> {
        let _guard = crate::iroh_node::IROH_TEST_LOCK.lock().await;
        let directory = tempfile::tempdir()?;
        let owner = IrohNode::persistent(directory.path().join("owner")).await?;
        let source = owner.create_workspace().await?;
        let source_ticket = source.share(true).await?;
        let active = IrohNode::persistent(directory.path().join("active")).await?;
        let active_source = active.import_workspace(&source_ticket).await?;
        let retired = IrohNode::persistent(directory.path().join("retired")).await?;
        let retired_source = retired.import_workspace(&source_ticket).await?;

        let source_records = WorkspaceRecords::new(&source);
        let (devices, owner_actor) =
            seed_source(&active, &active_source, &retired, &retired_source, &source).await?;

        let retired_endpoint = retired.endpoint_id().to_string();
        let mut retired_device = devices
            .into_iter()
            .find(|device| device.endpoint_id == retired_endpoint)
            .context("retired device is absent")?;
        retired_device.retired_at = Some(Hlc {
            physical_ms: 200,
            logical: 0,
            actor_id: owner_actor,
        });
        source_records
            .put_device(&retired_device)
            .await
            .context("store retirement")?;

        let rotation = rotate_workspace(&owner, &source, 300)
            .await
            .context("rotate workspace")?;
        assert_eq!(rotation.archived_workspace_id, source.id().to_string());
        assert_ne!(rotation.workspace_id, rotation.archived_workspace_id);
        assert_eq!(
            rotation.reinvite_endpoints,
            vec![active.endpoint_id().to_string()]
        );
        assert_eq!(
            (
                rotation.copied_notes,
                rotation.copied_assets,
                rotation.copied_configs
            ),
            (1, 1, 1)
        );
        assert!(
            retired
                .open_workspace_str(&rotation.workspace_id)
                .await
                .is_err(),
            "retired peer unexpectedly has the rotated namespace capability"
        );
        let active_target = active
            .import_workspace(&rotation.writable_ticket)
            .await
            .context("reinvite active peer")?;
        wait_for_checkpoint(WorkspaceRecords::new(&active_target)).await?;
        WorkspaceRecords::new(&active_target)
            .put_device(&device(
                &active,
                ActorId::new(active_target.author_id().to_string()),
                "active",
            ))
            .await
            .context("register reinvited peer")?;

        retired_source
            .put("attack/probe", "old namespace write")
            .await
            .context("write to archived namespace")?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let target = owner
            .open_workspace_str(&rotation.workspace_id)
            .await?
            .context("rotated workspace is absent")?;
        assert!(
            target
                .get("attack/probe")
                .await
                .context("read rotated namespace")?
                .is_none()
        );

        retired.shutdown().await?;
        active.shutdown().await?;
        owner.shutdown().await?;
        Ok(())
    }

    fn device(node: &IrohNode, author_id: ActorId, label: &str) -> DeviceRecord {
        DeviceRecord {
            schema: CURRENT_SCHEMA,
            endpoint_id: node.endpoint_id().to_string(),
            author_id,
            label: label.to_owned(),
            capabilities: BTreeSet::from(["write".to_owned()]),
            last_seen_ms: Some(100),
            retired_at: None,
        }
    }
}
