use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{self, Read as _, Write as _};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub const PLUGIN_DIRECTORY: &str = "plugins";

pub fn directory(config_file: &Path) -> Result<PathBuf> {
    Ok(config_file
        .parent()
        .context("xo config path has no parent")?
        .join(PLUGIN_DIRECTORY))
}

pub fn discover(directory: &Path) -> Result<BTreeMap<String, String>> {
    let mut plugins = BTreeMap::new();
    if !directory.exists() {
        return Ok(plugins);
    }
    for entry in std::fs::read_dir(directory)
        .with_context(|| format!("read plugin directory {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file()
            || path.extension().and_then(|value| value.to_str()) != Some("scm")
        {
            continue;
        }
        let name = path
            .file_name()
            .context("plugin has no file name")?
            .to_string_lossy();
        let logical_path = format!("plugins/{name}");
        if !xo_core::steel_runtime::valid_plugin_path(&logical_path) {
            continue;
        }
        plugins.insert(
            logical_path,
            std::fs::read_to_string(&path)
                .with_context(|| format!("read plugin {}", path.display()))?,
        );
    }
    Ok(plugins)
}

pub fn names(directory: &Path) -> Result<Vec<String>> {
    Ok(discover(directory)?
        .keys()
        .filter_map(|path| {
            Path::new(path)
                .file_stem()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect())
}

pub fn install(directory: &Path, name: &str, source: &Path, replace: bool) -> Result<PathBuf> {
    validate_name(name)?;
    let path = directory.join(format!("{name}.scm"));
    if path.exists() != replace {
        if replace {
            bail!("plugin {name:?} is not installed");
        }
        bail!("plugin {name:?} is already installed; use `xo plugin update`");
    }

    let source = read_source(source)?;
    let logical_path = format!("plugins/{name}.scm");
    xo_core::steel_runtime::validate_plugin(&logical_path, &source)
        .with_context(|| format!("validate plugin {name:?}"))?;

    std::fs::create_dir_all(directory)
        .with_context(|| format!("create plugin directory {}", directory.display()))?;
    let temporary = directory.join(format!(".{name}.scm.tmp"));
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .with_context(|| format!("write temporary plugin {}", temporary.display()))?;
    file.write_all(source.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, &path)
        .with_context(|| format!("install plugin {}", path.display()))?;
    Ok(path)
}

pub fn remove(directory: &Path, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    let path = directory.join(format!("{name}.scm"));
    if !path.is_file() {
        bail!("plugin {name:?} is not installed");
    }
    std::fs::remove_file(&path).with_context(|| format!("remove plugin {}", path.display()))?;
    Ok(path)
}

fn read_source(path: &Path) -> Result<String> {
    let mut source = String::new();
    if path == Path::new("-") {
        io::stdin().read_to_string(&mut source)?;
    } else {
        source = std::fs::read_to_string(path)
            .with_context(|| format!("read plugin source {}", path.display()))?;
    }
    Ok(source)
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("plugin name must contain only ASCII letters, digits, - and _");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLUGIN: &str = r#"
        (define (xo-plugin-manifest)
          "{\"schema\":1,\"actions\":[{\"id\":\"example-action\",\"description\":\"Example\"}]}")
        (define (xo-plugin-run input)
          "{\"choices\":[]}")
    "#;

    #[test]
    fn installs_discovers_updates_and_removes_plugins() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source.scm");
        let plugins = fixture.path().join("plugins");
        std::fs::write(&source, PLUGIN).unwrap();

        let installed = install(&plugins, "example", &source, false).unwrap();
        assert_eq!(installed, plugins.join("example.scm"));
        assert_eq!(names(&plugins).unwrap(), vec!["example"]);
        assert!(
            discover(&plugins)
                .unwrap()
                .contains_key("plugins/example.scm")
        );
        assert!(install(&plugins, "example", &source, false).is_err());
        install(&plugins, "example", &source, true).unwrap();
        assert_eq!(remove(&plugins, "example").unwrap(), installed);
        assert!(names(&plugins).unwrap().is_empty());
    }

    #[test]
    fn rejects_unsafe_names_and_invalid_manifests() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("invalid.scm");
        std::fs::write(&source, "(define value 1)").unwrap();
        assert!(install(fixture.path(), "../escape", &source, false).is_err());
        assert!(install(fixture.path(), "invalid", &source, false).is_err());
    }
}
