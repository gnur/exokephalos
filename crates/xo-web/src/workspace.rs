use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use xo_core::behavior::{Query, WorkspaceBehavior, default_views};
use xo_core::domain::{Frontmatter, FrontmatterValue};
use xo_core::steel_runtime::SteelWorkspace;
use xo_core::{
    ActorId, CURRENT_SCHEMA, ConfigRevision, Conflict, DeviceRecord, Head, Hlc, HlcClock, Note,
    NoteId, NoteRevision, RevisionId, resolve_heads, validate_revision_graph,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    key: String,
    value_base64: String,
    author: String,
}

impl Entry {
    fn bytes(&self) -> Result<Vec<u8>> {
        BASE64
            .decode(&self.value_base64)
            .context("decode document value")
    }
}

#[derive(Default)]
struct NoteGroup {
    revisions: BTreeMap<RevisionId, NoteRevision>,
    heads: Vec<Head>,
}

struct Repository {
    groups: BTreeMap<NoteId, NoteGroup>,
    behavior: WorkspaceBehavior,
    diagnostics: Vec<String>,
}

struct DecodedRecords {
    groups: BTreeMap<NoteId, NoteGroup>,
    configs: Vec<(ConfigRevision, RevisionId)>,
    diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserNote {
    id: String,
    frontmatter: Frontmatter,
    body: String,
    path: String,
    markdown: String,
    winning_revision: String,
    conflict: Option<Conflict>,
    history: Vec<HistoryRevision>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryRevision {
    id: String,
    author: String,
    physical_ms: u64,
    deleted: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceSnapshot {
    behavior: WorkspaceBehavior,
    notes: Vec<BrowserNote>,
    deleted: Vec<BrowserNote>,
    conflicts: usize,
    diagnostics: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserQuery {
    view: String,
    subview: Option<String>,
    #[serde(default)]
    search: String,
    #[serde(default)]
    tags: BTreeSet<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NoteMutation {
    operation: MutationOperation,
    note_id: Option<String>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    markdown: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum MutationOperation {
    Save,
    Delete,
    Restore,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreparedMutation {
    note_id: String,
    writes: Vec<PreparedWrite>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreparedWrite {
    key: String,
    value_base64: String,
}

pub fn snapshot_json(entries_json: &str) -> Result<String> {
    let entries: Vec<Entry> =
        serde_json::from_str(entries_json).context("decode document entries")?;
    let repository = Repository::load(&entries)?;
    serde_json::to_string(&repository.snapshot()?).context("encode workspace snapshot")
}

pub fn query_snapshot_json(snapshot_json: &str, query_json: &str) -> Result<String> {
    let snapshot: WorkspaceSnapshot =
        serde_json::from_str(snapshot_json).context("decode workspace snapshot")?;
    let requested: BrowserQuery = serde_json::from_str(query_json).context("decode note query")?;
    let notes = snapshot
        .notes
        .iter()
        .map(|note| Note {
            id: NoteId::new(&note.id),
            frontmatter: note.frontmatter.clone(),
            body: note.body.clone(),
            path: note.path.clone(),
        })
        .collect::<Vec<_>>();
    let selected = snapshot.behavior.query(
        &notes,
        &Query {
            view: requested.view,
            subview: requested.subview,
            title: (!requested.search.is_empty()).then_some(requested.search),
            tags: requested.tags,
            limit: None,
        },
    )?;
    let by_id = snapshot
        .notes
        .into_iter()
        .map(|note| (note.id.clone(), note))
        .collect::<BTreeMap<_, _>>();
    let result = selected
        .into_iter()
        .filter_map(|note| by_id.get(note.id.as_str()).cloned())
        .collect::<Vec<_>>();
    serde_json::to_string(&result).context("encode note query")
}

pub fn prepare_mutation_json(
    entries_json: &str,
    author: &str,
    input_json: &str,
    now_ms: u64,
) -> Result<String> {
    let entries: Vec<Entry> =
        serde_json::from_str(entries_json).context("decode document entries")?;
    let repository = Repository::load(&entries)?;
    let input: NoteMutation = serde_json::from_str(input_json).context("decode note mutation")?;
    let author = ActorId::new(author);
    let (note_id, frontmatter, body, deleted) = mutation_contents(&repository, &input, now_ms)?;

    let mut predecessors = BTreeSet::new();
    if let Some(current) = repository.resolve(&note_id)? {
        predecessors.insert(current.winning_revision);
        predecessors.extend(
            current
                .conflict
                .into_iter()
                .flat_map(|value| value.concurrent_revisions),
        );
    }
    let mut clock = HlcClock::new(author.clone());
    let mut timestamps = repository
        .groups
        .values()
        .flat_map(|group| {
            group
                .revisions
                .values()
                .map(|revision| revision.hlc.clone())
        })
        .collect::<Vec<_>>();
    timestamps.sort();
    for timestamp in timestamps {
        clock.observe(&timestamp, now_ms);
    }
    let revision = NoteRevision {
        schema: CURRENT_SCHEMA,
        note_id: note_id.clone(),
        frontmatter,
        body,
        materialized_path: xo_core::markdown::canonical_note_path(&note_id, &BTreeMap::new()),
        hlc: clock.next(now_ms),
        author_id: author.clone(),
        predecessors,
        deleted,
    };
    let mut revision = revision;
    revision.materialized_path =
        xo_core::markdown::canonical_note_path(&note_id, &revision.frontmatter);
    let revision_id = revision.id()?;
    let head = Head {
        note_id: note_id.clone(),
        author_id: author,
        revision_id: revision_id.clone(),
    };
    let writes = vec![
        PreparedWrite {
            key: format!("note/{note_id}/revision/{revision_id}"),
            value_base64: BASE64.encode(revision.canonical_bytes()?),
        },
        PreparedWrite {
            key: format!("note/{note_id}/head/{}", head.author_id),
            value_base64: BASE64.encode(encode(&head)?),
        },
    ];
    serde_json::to_string(&PreparedMutation {
        note_id: note_id.to_string(),
        writes,
    })
    .context("encode note mutation")
}

fn mutation_contents(
    repository: &Repository,
    input: &NoteMutation,
    now_ms: u64,
) -> Result<(NoteId, Frontmatter, String, bool)> {
    let note_id = input.note_id.as_deref().map(NoteId::new);
    let resolved = match note_id.as_ref() {
        Some(id) => repository.resolve(id)?,
        None => None,
    };
    if matches!(
        input.operation,
        MutationOperation::Delete | MutationOperation::Restore
    ) {
        let id = note_id.context("note ID is required")?;
        let current = resolved.context("note is unavailable")?;
        let revision = repository.groups[&id]
            .revisions
            .get(&current.winning_revision)
            .context("winning revision is unavailable")?;
        return Ok((
            id,
            revision.frontmatter.clone(),
            revision.body.clone(),
            matches!(input.operation, MutationOperation::Delete),
        ));
    }
    let parsed = xo_core::markdown::parse(&input.markdown)?;
    match (note_id, resolved) {
        (Some(id), Some(_)) => Ok((
            id,
            parsed.frontmatter.unwrap_or_default(),
            parsed.body,
            false,
        )),
        (None, None) => {
            let instant =
                time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(now_ms) * 1_000_000)?;
            let id = NoteId::new(xo_core::id::generate(instant));
            let mut frontmatter = parsed.frontmatter.unwrap_or_default();
            let title = string_field(&frontmatter, "title")
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(input.title.as_str())
                .trim();
            frontmatter.insert("title".into(), FrontmatterValue::String(title.into()));
            let created = instant.format(&time::format_description::well_known::Rfc3339)?;
            let frontmatter =
                xo_core::markdown::required_frontmatter(frontmatter, id.as_str(), &created);
            Ok((id, frontmatter, parsed.body, false))
        }
        (Some(_), None) => bail!("note is unavailable"),
        (None, Some(_)) => unreachable!(),
    }
}

fn decode_records(entries: &[Entry]) -> Result<DecodedRecords> {
    let mut groups = BTreeMap::<NoteId, NoteGroup>::new();
    let mut config_records = Vec::new();
    let mut diagnostics = Vec::new();
    let cutoffs = retirement_cutoffs(entries, &mut diagnostics)?;
    for entry in entries {
        let parts = entry.key.split('/').collect::<Vec<_>>();
        if parts.first() == Some(&"note") && parts.len() == 4 {
            let note_id = NoteId::new(parts[1]);
            match parts[2] {
                "revision" => {
                    let revision_id = RevisionId::new(parts[3]);
                    let revision: NoteRevision = decode(&entry.bytes()?)?;
                    if revision.note_id != note_id
                        || revision.author_id.as_str() != entry.author
                        || revision.id()? != revision_id
                        || !allows(&cutoffs, &revision.author_id, &revision.hlc)
                    {
                        diagnostics.push(format!("rejected mismatched record {}", entry.key));
                    } else {
                        groups
                            .entry(note_id)
                            .or_default()
                            .revisions
                            .insert(revision_id, revision);
                    }
                }
                "head" => {
                    let head: Head = decode(&entry.bytes()?)?;
                    if head.note_id != note_id
                        || head.author_id.as_str() != entry.author
                        || head.author_id.as_str() != parts[3]
                    {
                        diagnostics.push(format!("rejected mismatched record {}", entry.key));
                    } else {
                        groups.entry(note_id).or_default().heads.push(head);
                    }
                }
                _ => {}
            }
        } else if parts.first() == Some(&"config") {
            let record: ConfigRevision = decode(&entry.bytes()?)?;
            let revision_id = record.id()?;
            if entry.key == format!("config/{}/{revision_id}", record.path)
                && record.author_id.as_str() == entry.author
                && allows(&cutoffs, &record.author_id, &record.hlc)
            {
                config_records.push((record, revision_id));
            } else {
                diagnostics.push(format!("rejected mismatched record {}", entry.key));
            }
        }
    }
    Ok(DecodedRecords {
        groups,
        configs: config_records,
        diagnostics,
    })
}

fn retirement_cutoffs(
    entries: &[Entry],
    diagnostics: &mut Vec<String>,
) -> Result<BTreeMap<ActorId, Hlc>> {
    let mut cutoffs = BTreeMap::<ActorId, Hlc>::new();
    for entry in entries
        .iter()
        .filter(|entry| entry.key.starts_with("device/"))
    {
        let device: DeviceRecord = decode(&entry.bytes()?)?;
        let expected_signer = device
            .retired_at
            .as_ref()
            .map_or(&device.author_id, |cutoff| &cutoff.actor_id);
        if entry.key != format!("device/{}", device.endpoint_id)
            || expected_signer.as_str() != entry.author
            || device.validate().is_err()
        {
            diagnostics.push(format!("rejected mismatched record {}", entry.key));
            continue;
        }
        let Some(cutoff) = device.retired_at else {
            continue;
        };
        cutoffs
            .entry(device.author_id)
            .and_modify(|current| {
                if (cutoff.physical_ms, cutoff.logical) < (current.physical_ms, current.logical) {
                    *current = cutoff.clone();
                }
            })
            .or_insert(cutoff);
    }
    Ok(cutoffs)
}

fn allows(cutoffs: &BTreeMap<ActorId, Hlc>, author: &ActorId, timestamp: &Hlc) -> bool {
    cutoffs.get(author).is_none_or(|cutoff| {
        (timestamp.physical_ms, timestamp.logical) <= (cutoff.physical_ms, cutoff.logical)
    })
}

fn load_behavior(
    entries: &[Entry],
    config_records: Vec<(ConfigRevision, RevisionId)>,
    diagnostics: &mut Vec<String>,
) -> Result<WorkspaceBehavior> {
    let values = entries
        .iter()
        .map(|entry| (entry.key.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut configs = BTreeMap::<String, (ConfigRevision, RevisionId, String)>::new();
    for (record, revision_id) in config_records {
        let blob_key = format!("config-blob/{revision_id}");
        let Some(blob) = values.get(blob_key.as_str()) else {
            diagnostics.push(format!(
                "configuration content is unavailable: {}",
                record.path
            ));
            continue;
        };
        let bytes = blob.bytes()?;
        if u64::try_from(bytes.len()).ok() != Some(record.size)
            || blake3::hash(&bytes).to_hex().as_str() != record.blob_hash
        {
            diagnostics.push(format!(
                "configuration content does not match: {}",
                record.path
            ));
            continue;
        }
        let source = String::from_utf8(bytes)
            .with_context(|| format!("configuration {} is not UTF-8", record.path))?;
        let replace = configs
            .get(&record.path)
            .is_none_or(|(current, current_id, _)| {
                (&record.hlc, &revision_id) > (&current.hlc, current_id)
            });
        if replace {
            configs.insert(record.path.clone(), (record, revision_id, source));
        }
    }
    let mut modules = BTreeMap::new();
    let mut main = None;
    for (path, (_, _, source)) in configs {
        if path == "xo.scm" {
            main = Some(source);
        } else {
            modules.insert(path, source);
        }
    }
    let mut behavior = match main {
        Some(source) => SteelWorkspace::load(&source, &modules, "1970-01-01T00:00:00Z")
            .unwrap_or_else(|error| {
                diagnostics.push(format!("workspace configuration: {error}"));
                WorkspaceBehavior::default()
            }),
        None => WorkspaceBehavior::default(),
    };
    if behavior.views.is_empty() {
        behavior.default_view = "notes".into();
        behavior.views = default_views();
    }
    behavior.validate()?;
    Ok(behavior)
}

impl Repository {
    fn load(entries: &[Entry]) -> Result<Self> {
        let decoded = decode_records(entries)?;
        let mut diagnostics = decoded.diagnostics;
        let behavior = load_behavior(entries, decoded.configs, &mut diagnostics)?;
        Ok(Self {
            groups: decoded.groups,
            behavior,
            diagnostics,
        })
    }

    fn resolve(&self, id: &NoteId) -> Result<Option<xo_core::ResolvedNote>> {
        let Some(group) = self.groups.get(id) else {
            return Ok(None);
        };
        validate_revision_graph(&group.revisions)?;
        Ok(resolve_heads(&group.revisions, &group.heads))
    }

    fn notes(&self, deleted: bool) -> Result<Vec<BrowserNote>> {
        let mut output = Vec::new();
        for (id, group) in &self.groups {
            let Some(resolved) = self.resolve(id)? else {
                continue;
            };
            let Some(winner) = group.revisions.get(&resolved.winning_revision) else {
                continue;
            };
            if winner.deleted != deleted {
                continue;
            }
            let mut history = group
                .revisions
                .iter()
                .map(|(revision_id, revision)| HistoryRevision {
                    id: revision_id.to_string(),
                    author: revision.author_id.to_string(),
                    physical_ms: revision.hlc.physical_ms,
                    deleted: revision.deleted,
                })
                .collect::<Vec<_>>();
            history.sort_by_key(|item| item.physical_ms);
            output.push(BrowserNote {
                id: id.to_string(),
                frontmatter: winner.frontmatter.clone(),
                body: winner.body.clone(),
                path: winner.materialized_path.clone(),
                markdown: xo_core::markdown::render(&winner.frontmatter, &winner.body)?,
                winning_revision: resolved.winning_revision.to_string(),
                conflict: resolved.conflict,
                history,
            });
        }
        output.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(output)
    }

    fn snapshot(&self) -> Result<WorkspaceSnapshot> {
        let notes = self.notes(false)?;
        Ok(WorkspaceSnapshot {
            conflicts: notes.iter().filter(|note| note.conflict.is_some()).count(),
            behavior: self.behavior.clone(),
            notes,
            deleted: self.notes(true)?,
            diagnostics: self.diagnostics.clone(),
        })
    }
}

fn string_field<'a>(frontmatter: &'a Frontmatter, field: &str) -> Option<&'a str> {
    match frontmatter.get(field) {
        Some(FrontmatterValue::String(value)) => Some(value),
        _ => None,
    }
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    ciborium::from_reader(Cursor::new(bytes)).context("decode CBOR record")
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).context("encode CBOR record")?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(entries: &mut BTreeMap<String, Entry>, prepared: PreparedMutation, author: &str) {
        for write in prepared.writes {
            entries.insert(
                write.key.clone(),
                Entry {
                    key: write.key,
                    value_base64: write.value_base64,
                    author: author.to_owned(),
                },
            );
        }
    }

    fn entries_json(entries: &BTreeMap<String, Entry>) -> String {
        serde_json::to_string(
            &entries
                .values()
                .map(|entry| {
                    serde_json::json!({
                        "key": entry.key,
                        "valueBase64": entry.value_base64,
                        "author": entry.author,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn install_config(
        entries: &mut BTreeMap<String, Entry>,
        behavior: &WorkspaceBehavior,
        author: &str,
    ) {
        let source = xo_core::steel_runtime::encode_config(behavior, false);
        let record = ConfigRevision {
            schema: CURRENT_SCHEMA,
            path: "xo.scm".into(),
            blob_hash: blake3::hash(source.as_bytes()).to_hex().to_string(),
            size: source.len() as u64,
            hlc: xo_core::Hlc {
                physical_ms: 1,
                logical: 0,
                actor_id: ActorId::new(author),
            },
            author_id: ActorId::new(author),
            predecessors: BTreeSet::new(),
        };
        let revision_id = record.id().unwrap();
        entries.insert(
            format!("config/xo.scm/{revision_id}"),
            Entry {
                key: format!("config/xo.scm/{revision_id}"),
                value_base64: BASE64.encode(encode(&record).unwrap()),
                author: author.into(),
            },
        );
        entries.insert(
            format!("config-blob/{revision_id}"),
            Entry {
                key: format!("config-blob/{revision_id}"),
                value_base64: BASE64.encode(source),
                author: author.into(),
            },
        );
    }

    #[test]
    fn creates_queries_edits_and_deletes_authoritative_notes() {
        let author = "browser-author";
        let mut entries = BTreeMap::new();
        let created: PreparedMutation = serde_json::from_str(&prepare_mutation_json(
            "[]",
            author,
            r#"{"operation":"save","title":"Browser note","markdown":"---\ntitle: Browser note\ntype: note\ntags: [web]\n---\nFirst body"}"#,
            1_800_000_000_000,
        ).unwrap()).unwrap();
        let note_id = created.note_id.clone();
        apply(&mut entries, created, author);

        let encoded = entries_json(&entries);
        let snapshot: serde_json::Value =
            serde_json::from_str(&snapshot_json(&encoded).unwrap()).unwrap();
        assert_eq!(snapshot["behavior"]["default_view"], "notes");
        assert_eq!(snapshot["notes"][0]["frontmatter"]["title"], "Browser note");
        let query = query_snapshot_json(
            &snapshot_json(&encoded).unwrap(),
            r#"{"view":"notes","search":"browser","tags":["web"]}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&query)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let edited: PreparedMutation = serde_json::from_str(
            &prepare_mutation_json(
                &encoded,
                author,
                &serde_json::json!({
                    "operation": "save",
                    "noteId": note_id,
                    "markdown": "---\ntitle: Edited\ntype: note\ntags: [web]\n---\nSecond body",
                })
                .to_string(),
                1_800_000_000_001,
            )
            .unwrap(),
        )
        .unwrap();
        apply(&mut entries, edited, author);
        let encoded = entries_json(&entries);
        let snapshot: serde_json::Value =
            serde_json::from_str(&snapshot_json(&encoded).unwrap()).unwrap();
        assert_eq!(snapshot["notes"][0]["body"], "Second body");
        assert_eq!(snapshot["notes"][0]["history"].as_array().unwrap().len(), 2);

        let deleted: PreparedMutation = serde_json::from_str(
            &prepare_mutation_json(
                &encoded,
                author,
                &serde_json::json!({ "operation": "delete", "noteId": note_id }).to_string(),
                1_800_000_000_002,
            )
            .unwrap(),
        )
        .unwrap();
        apply(&mut entries, deleted, author);
        let snapshot: serde_json::Value =
            serde_json::from_str(&snapshot_json(&entries_json(&entries)).unwrap()).unwrap();
        assert!(snapshot["notes"].as_array().unwrap().is_empty());
        assert_eq!(snapshot["deleted"][0]["frontmatter"]["title"], "Edited");
    }

    #[test]
    fn retired_author_writes_after_the_cutoff_are_hidden() {
        let author = "browser-author";
        let mut entries = BTreeMap::new();
        let created: PreparedMutation = serde_json::from_str(
            &prepare_mutation_json(
                "[]",
                author,
                r#"{"operation":"save","title":"Rejected","markdown":"---\ntitle: Rejected\ntype: note\n---\n"}"#,
                2_000,
            )
            .unwrap(),
        )
        .unwrap();
        apply(&mut entries, created, author);
        let device = DeviceRecord {
            schema: CURRENT_SCHEMA,
            endpoint_id: "retired-endpoint".into(),
            author_id: ActorId::new(author),
            label: "Retired browser".into(),
            capabilities: BTreeSet::new(),
            last_seen_ms: None,
            retired_at: Some(Hlc {
                physical_ms: 1_999,
                logical: 0,
                actor_id: ActorId::new("administrator"),
            }),
        };
        entries.insert(
            "device/retired-endpoint".into(),
            Entry {
                key: "device/retired-endpoint".into(),
                value_base64: BASE64.encode(encode(&device).unwrap()),
                author: "administrator".into(),
            },
        );
        let snapshot: serde_json::Value =
            serde_json::from_str(&snapshot_json(&entries_json(&entries)).unwrap()).unwrap();
        assert!(snapshot["notes"].as_array().unwrap().is_empty());
    }

    #[test]
    fn configured_views_and_subviews_filter_in_rust() {
        use xo_core::behavior::{Predicate, SubviewDescriptor, ViewDescriptor};

        let author = "browser-author";
        let behavior = WorkspaceBehavior {
            schema: 1,
            default_view: "library".into(),
            views: vec![ViewDescriptor {
                id: "library".into(),
                name: "Library".into(),
                key: Some("l".into()),
                show_tags: true,
                title_field: "title".into(),
                subtitle_field: Some("status".into()),
                sort_field: Some("title".into()),
                descending: false,
                preview: None,
                predicate: Predicate::FieldEquals {
                    field: "type".into(),
                    value: "book".into(),
                },
                subviews: vec![SubviewDescriptor {
                    id: "reading".into(),
                    name: "Reading".into(),
                    predicate: Predicate::HasTag {
                        tag: "reading".into(),
                    },
                }],
            }],
            actions: vec![],
            templates: vec![],
            capability_grants: BTreeMap::new(),
            query_limit: 500,
        };
        let mut entries = BTreeMap::new();
        install_config(&mut entries, &behavior, author);
        for (title, tags) in [("Current", "[reading]"), ("Finished", "[read]")] {
            let prepared: PreparedMutation = serde_json::from_str(
                &prepare_mutation_json(
                    &entries_json(&entries),
                    author,
                    &serde_json::json!({
                        "operation": "save",
                        "title": title,
                        "markdown": format!("---\ntitle: {title}\ntype: book\ntags: {tags}\n---\n"),
                    })
                    .to_string(),
                    1_800_000_000_000,
                )
                .unwrap(),
            )
            .unwrap();
            apply(&mut entries, prepared, author);
        }
        let encoded = entries_json(&entries);
        let snapshot: serde_json::Value =
            serde_json::from_str(&snapshot_json(&encoded).unwrap()).unwrap();
        assert_eq!(
            snapshot["behavior"]["views"][0]["subviews"][0]["name"],
            "Reading"
        );
        let result: serde_json::Value = serde_json::from_str(
            &query_snapshot_json(
                &snapshot_json(&encoded).unwrap(),
                r#"{"view":"library","subview":"reading","search":"","tags":[]}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(result.as_array().unwrap().len(), 1);
        assert_eq!(result[0]["frontmatter"]["title"], "Current");
    }
}
