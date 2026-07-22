use xo_core::domain::FrontmatterValue;
use xo_core::{AssetRecord, markdown};

const VALID_NOTE: &str = include_str!("fixtures/compat/valid-note.md");
const ENCRYPTED_NOTE: &str = include_str!("fixtures/compat/encrypted-note.md");
const MALFORMED_NOTE: &str = include_str!("fixtures/compat/malformed-frontmatter.md");
const ASSET_RECORD: &str = include_str!("fixtures/compat/asset-record.json");
const FENNEL_CONFIG: &str = include_str!("fixtures/compat/exo.fnl");

#[test]
fn representative_markdown_round_trips_semantically() {
    let parsed = markdown::parse(VALID_NOTE).unwrap();
    let frontmatter = parsed.frontmatter.unwrap();
    assert_eq!(
        frontmatter.get("id"),
        Some(&FrontmatterValue::String("note002".to_owned()))
    );
    let rendered = markdown::render(&frontmatter, &parsed.body).unwrap();
    assert_eq!(
        markdown::parse(&rendered).unwrap().frontmatter,
        Some(frontmatter)
    );
}

#[test]
fn deterministic_envelope_decrypts_with_legacy_contract() {
    let parsed = markdown::parse(ENCRYPTED_NOTE).unwrap();
    assert_eq!(
        xo_core::encryption::decrypt("note001", "hunter2", parsed.body.trim()).unwrap(),
        "secret"
    );
}

#[test]
fn malformed_frontmatter_is_diagnosed() {
    assert!(markdown::parse(MALFORMED_NOTE).is_err());
}

#[test]
fn asset_fixture_uses_valid_schema() {
    let record: AssetRecord = serde_json::from_str(ASSET_RECORD).unwrap();
    record.validate().unwrap();
}

#[test]
fn legacy_configuration_is_retained_for_the_future_migrator() {
    assert!(FENNEL_CONFIG.contains(":default-view"));
    assert!(FENNEL_CONFIG.contains(":mark-done"));
}
