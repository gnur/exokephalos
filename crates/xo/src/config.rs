use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const CONFIG_RELATIVE_PATH: &str = ".config/xo/config.scm";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XoConfig {
    pub schema: u16,
    pub state_dir: PathBuf,
    #[serde(default)]
    pub workspace: Option<String>,
    pub projection: PathBuf,
    pub pwa_url: String,
}

const fn schema() -> u16 {
    2
}

impl Default for XoConfig {
    fn default() -> Self {
        Self {
            schema: schema(),
            state_dir: PathBuf::from("~/.local/share/xo"),
            workspace: None,
            projection: PathBuf::from("~/notes"),
            pwa_url: "https://xo.exokephalos.dev/".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CliOverrides {
    pub state_dir: Option<PathBuf>,
    pub workspace: Option<String>,
    pub projection: Option<PathBuf>,
}

impl XoConfig {
    pub fn load(path: &Path, home: &Path) -> Result<Self> {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                bail!(
                    "configuration file {} is missing; initialize it with:\n  mkdir -p ~/.config/xo\n  xo config-init > ~/.config/xo/config.scm",
                    path.display()
                );
            }
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        let json = xo_core::steel_runtime::evaluate_xo_config(&source)?;
        let mut config: Self = serde_json::from_str(&json).context("decode xo configuration")?;
        if config.schema != schema() {
            bail!("unsupported xo configuration schema {}", config.schema);
        }
        validate_pwa_url(&config.pwa_url)?;
        config.state_dir = expand_home(&config.state_dir, home);
        config.projection = expand_home(&config.projection, home);
        Ok(config)
    }

    #[must_use]
    pub fn apply(mut self, overrides: CliOverrides, home: &Path) -> Self {
        if let Some(value) = overrides.state_dir {
            self.state_dir = expand_home(&value, home);
        }
        if let Some(value) = overrides.workspace {
            self.workspace = Some(value);
        }
        if let Some(value) = overrides.projection {
            self.projection = expand_home(&value, home);
        }
        self
    }

    pub fn document(&self) -> Result<String> {
        let string = |value: &str| serde_json::to_string(value);
        let optional = |value: Option<&str>| -> Result<String, serde_json::Error> {
            value.map_or_else(|| Ok("#f".to_owned()), string)
        };
        Ok(format!(
            "; xo command defaults; command-line flags override these values.\n\
             (xo-config\n\
             \x20 (schema {})\n\
             \x20 (state-dir {})\n\
             \x20 (workspace {})\n\
             \x20 (projection {})\n\
             \x20 (pwa-url {}))\n",
            self.schema,
            string(&self.state_dir.to_string_lossy())?,
            optional(self.workspace.as_deref())?,
            string(&self.projection.to_string_lossy())?,
            string(&self.pwa_url)?,
        ))
    }
}

fn validate_pwa_url(value: &str) -> Result<()> {
    let url = url::Url::parse(value).context("parse pwa-url")?;
    if url.scheme() != "https" || url.host_str().is_none() {
        bail!("pwa-url must be an absolute HTTPS URL");
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("pwa-url must be an HTTPS origin without credentials, a path, query, or fragment");
    }
    Ok(())
}

pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .context("HOME is not set; cannot locate ~/.config/xo/config.scm")
}

#[must_use]
pub fn config_path(home: &Path) -> PathBuf {
    home.join(CONFIG_RELATIVE_PATH)
}

fn expand_home(path: &Path, home: &Path) -> PathBuf {
    if path == Path::new("~") {
        return home.to_path_buf();
    }
    path.strip_prefix(Path::new("~").join(OsStr::new("")))
        .map_or_else(|_| path.to_path_buf(), |suffix| home.join(suffix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_document_round_trips_and_expands_home() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("config.scm");
        std::fs::write(&path, XoConfig::default().document()?)?;
        let loaded = XoConfig::load(&path, Path::new("/home/tester"))?;
        assert_eq!(loaded.state_dir, Path::new("/home/tester/.local/share/xo"));
        assert_eq!(loaded.projection, Path::new("/home/tester/notes"));
        assert_eq!(loaded.pwa_url, "https://xo.exokephalos.dev/");
        let document = std::fs::read_to_string(path)?;
        assert!(document.contains("(state-dir \"~/.local/share/xo\")"));
        assert!(!document.contains("(ticket "));
        assert!(!document.contains("{\\\"schema\\\""));
        Ok(())
    }

    #[test]
    fn pwa_url_accepts_custom_https_hosts_and_rejects_unsafe_values() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("config.scm");
        let config = XoConfig {
            pwa_url: "https://notes.example.test/".into(),
            ..XoConfig::default()
        };
        std::fs::write(&path, config.document()?)?;
        assert_eq!(
            XoConfig::load(&path, Path::new("/home/tester"))?.pwa_url,
            "https://notes.example.test/"
        );

        let invalid = XoConfig {
            pwa_url: "http://notes.example.test/".into(),
            ..XoConfig::default()
        };
        std::fs::write(&path, invalid.document()?)?;
        assert!(XoConfig::load(&path, Path::new("/home/tester")).is_err());
        Ok(())
    }

    #[test]
    fn command_line_values_override_file_values() {
        let config = XoConfig::default().apply(
            CliOverrides {
                state_dir: Some("~/state".into()),
                workspace: Some("workspace-id".into()),
                projection: Some("~/knowledge".into()),
            },
            Path::new("/users/alice"),
        );
        assert_eq!(config.state_dir, Path::new("/users/alice/state"));
        assert_eq!(config.workspace.as_deref(), Some("workspace-id"));
        assert_eq!(config.projection, Path::new("/users/alice/knowledge"));
    }

    #[test]
    fn missing_configuration_explains_initialization() {
        let error = XoConfig::load(
            Path::new("/definitely/missing/xo/config.scm"),
            Path::new("/home/tester"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("xo config-init > ~/.config/xo/config.scm"));
    }
}
