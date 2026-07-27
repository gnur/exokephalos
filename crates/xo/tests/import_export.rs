use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, ensure};
use xo::config::XoConfig;

#[test]
fn xo_recursively_imports_and_conventionally_exports_markdown() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let home = directory.path().join("home");
    let source = directory.path().join("source");
    let export = directory.path().join("export");
    std::fs::create_dir_all(home.join(".config/xo"))?;
    std::fs::create_dir_all(source.join("nested"))?;
    let first = "---\ntitle: Same title\ntags: [one]\n---\nFirst body\n";
    let second = "---\ntitle: Same title\ntags: [two]\n---\nSecond body\n";
    std::fs::write(source.join("first.md"), first)?;
    std::fs::write(source.join("nested/second.md"), second)?;
    let config = XoConfig {
        state_dir: directory.path().join("state"),
        projection: directory.path().join("projection"),
        ..XoConfig::default()
    };
    std::fs::write(
        home.join(".config/xo/config.scm"),
        config.document().context("render command config")?,
    )?;

    let imported = run_xo(&home, ["import", source.to_str().context("source path")?])?;
    ensure!(
        imported.status.success(),
        "xo import failed: {}",
        String::from_utf8_lossy(&imported.stderr)
    );
    ensure!(
        String::from_utf8_lossy(&imported.stdout).contains("imported=2"),
        "unexpected import output: {}",
        String::from_utf8_lossy(&imported.stdout)
    );
    ensure!(std::fs::read_to_string(source.join("first.md"))? == first);
    ensure!(std::fs::read_to_string(source.join("nested/second.md"))? == second);

    let exported = run_xo(
        &home,
        [
            "export",
            export.to_str().context("export path")?,
            "--type",
            "note",
        ],
    )?;
    ensure!(
        exported.status.success(),
        "xo export failed: {}",
        String::from_utf8_lossy(&exported.stderr)
    );
    ensure!(
        String::from_utf8_lossy(&exported.stdout).contains("exported=2"),
        "unexpected export output: {}",
        String::from_utf8_lossy(&exported.stdout)
    );

    let mut markdown = Vec::new();
    collect_markdown(&export, &mut markdown)?;
    markdown.sort();
    ensure!(markdown.len() == 2);
    ensure!(
        markdown
            .iter()
            .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
            .collect::<BTreeSet<_>>()
            == BTreeSet::from(["same-title-1.md", "same-title.md"])
    );
    for path in markdown {
        let document = xo_core::markdown::parse(&std::fs::read_to_string(path)?)?;
        let frontmatter = document.frontmatter.context("exported frontmatter")?;
        ensure!(!frontmatter.contains_key("id"));
        ensure!(!frontmatter.contains_key("type"));
        ensure!(!frontmatter.contains_key("created"));
    }
    Ok(())
}

fn run_xo<const N: usize>(home: &Path, args: [&str; N]) -> Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_xo"))
        .args(args)
        .env("HOME", home)
        .output()
        .context("run xo")
}

fn collect_markdown(directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_markdown(&path, output)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            output.push(path);
        }
    }
    Ok(())
}
