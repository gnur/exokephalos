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
    pub client_id: Option<String>,
    pub projection: PathBuf,
}

const fn schema() -> u16 {
    4
}

impl Default for XoConfig {
    fn default() -> Self {
        Self {
            schema: schema(),
            state_dir: PathBuf::from("~/.local/share/xo"),
            client_id: None,
            projection: PathBuf::from("~/notes"),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CliOverrides {
    pub state_dir: Option<PathBuf>,
    pub client_id: Option<String>,
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
        if let Some(client_id) = &config.client_id {
            xo_core::ClientId::parse(client_id.clone()).context("validate client-id")?;
        }
        config.state_dir = expand_home(&config.state_dir, home);
        config.projection = expand_home(&config.projection, home);
        Ok(config)
    }

    #[must_use]
    pub fn apply(mut self, overrides: CliOverrides, home: &Path) -> Self {
        if let Some(value) = overrides.state_dir {
            self.state_dir = expand_home(&value, home);
        }
        if let Some(value) = overrides.client_id {
            self.client_id = Some(value);
        }
        if let Some(value) = overrides.projection {
            self.projection = expand_home(&value, home);
        }
        self
    }

    pub fn resolved_client_id(&self) -> Result<xo_core::ClientId> {
        let value = self.client_id.clone().map_or_else(
            || {
                hostname::get()
                    .context("read system hostname")?
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("system hostname is not valid UTF-8"))
            },
            Ok,
        )?;
        xo_core::ClientId::parse(value).context("validate client-id")
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
             \x20 (client-id {})\n\
             \x20 (projection {}))\n",
            self.schema,
            string(&self.state_dir.to_string_lossy())?,
            optional(self.client_id.as_deref())?,
            string(&self.projection.to_string_lossy())?,
        ))
    }
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
        let document = std::fs::read_to_string(path)?;
        assert!(document.contains("(state-dir \"~/.local/share/xo\")"));
        assert!(!document.contains("(ticket "));
        assert!(!document.contains("{\\\"schema\\\""));
        Ok(())
    }

    #[test]
    fn command_line_values_override_file_values() {
        let config = XoConfig::default().apply(
            CliOverrides {
                state_dir: Some("~/state".into()),
                client_id: Some("alice-laptop".into()),
                projection: Some("~/knowledge".into()),
            },
            Path::new("/users/alice"),
        );
        assert_eq!(config.state_dir, Path::new("/users/alice/state"));
        assert_eq!(config.client_id.as_deref(), Some("alice-laptop"));
        assert_eq!(config.projection, Path::new("/users/alice/knowledge"));
    }

    #[test]
    fn schema_three_configuration_is_rejected() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("config.scm");
        std::fs::write(
            &path,
            XoConfig::default()
                .document()?
                .replace("(schema 4)", "(schema 3)"),
        )?;
        let error = XoConfig::load(&path, Path::new("/home/tester"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("unsupported xo configuration schema 3"));
        Ok(())
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
