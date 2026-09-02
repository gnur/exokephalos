use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

pub const DEFAULT_RELATIVE_PATH: &str = ".config/xo-syncd/config.scm";
const MAX_CONFIG_BYTES: usize = 64 * 1024;
const CURRENT_SCHEMA: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncdConfig {
    pub schema: u16,
    pub state_dir: Option<PathBuf>,
    pub bind: Option<String>,
    pub oidc_issuer: Option<String>,
    pub oidc_audience: Option<String>,
    pub oidc_client_id: Option<String>,
}

impl Default for SyncdConfig {
    fn default() -> Self {
        Self {
            schema: CURRENT_SCHEMA,
            state_dir: None,
            bind: None,
            oidc_issuer: None,
            oidc_audience: None,
            oidc_client_id: None,
        }
    }
}

impl SyncdConfig {
    pub fn load_optional(path: &Path, explicit: bool, home: Option<&Path>) -> Result<Self> {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !explicit => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        let mut config = Parser::new(&source).parse()?;
        if config.schema != CURRENT_SCHEMA {
            bail!(
                "unsupported xo-syncd configuration schema {}",
                config.schema
            );
        }
        if let (Some(home), Some(state_dir)) = (home, config.state_dir.as_mut()) {
            *state_dir = expand_home(state_dir, home);
        }
        Ok(config)
    }

    pub fn document(&self) -> String {
        let string = |value: &str| {
            serde_json::to_string(value).expect("configuration strings are serializable")
        };
        let optional = |value: Option<&str>| value.map_or_else(|| "#f".to_owned(), string);
        format!(
            "; xo-syncd server configuration; command-line flags override these values.\n\
             (xo-syncd-config\n\
             \x20 (schema {})\n\
             \x20 (state-dir {})\n\
             \x20 (bind {})\n\
             \x20 (oidc-issuer {})\n\
             \x20 (oidc-audience {})\n\
             \x20 (oidc-client-id {}))\n",
            self.schema,
            optional(self.state_dir.as_ref().and_then(|value| value.to_str())),
            optional(self.bind.as_deref()),
            optional(self.oidc_issuer.as_deref()),
            optional(self.oidc_audience.as_deref()),
            optional(self.oidc_client_id.as_deref()),
        )
    }
}

pub fn default_path(home: &Path) -> PathBuf {
    home.join(DEFAULT_RELATIVE_PATH)
}

fn expand_home(path: &Path, home: &Path) -> PathBuf {
    if path == Path::new("~") {
        return home.to_path_buf();
    }
    path.strip_prefix("~/")
        .map_or_else(|_| path.to_path_buf(), |suffix| home.join(suffix))
}

struct Parser<'a> {
    source: &'a str,
    position: usize,
}

impl<'a> Parser<'a> {
    const fn new(source: &'a str) -> Self {
        Self {
            source,
            position: 0,
        }
    }

    fn parse(mut self) -> Result<SyncdConfig> {
        if self.source.len() > MAX_CONFIG_BYTES {
            bail!("xo-syncd configuration exceeds {MAX_CONFIG_BYTES} bytes");
        }
        self.expect_char('(')?;
        self.expect_token("xo-syncd-config")?;
        let mut schema = None;
        let mut state_dir = None;
        let mut bind = None;
        let mut oidc_issuer = None;
        let mut oidc_audience = None;
        let mut oidc_client_id = None;
        loop {
            self.skip_ignored();
            if self.peek() == Some(')') {
                self.bump();
                break;
            }
            self.expect_char('(')?;
            let key = self.token()?;
            match key.as_str() {
                "schema" => set_once(&mut schema, self.integer()?, &key, self.position)?,
                "state-dir" => {
                    set_once(&mut state_dir, self.optional_string()?, &key, self.position)?;
                }
                "bind" => set_once(&mut bind, self.optional_string()?, &key, self.position)?,
                "oidc-issuer" => {
                    set_once(
                        &mut oidc_issuer,
                        self.optional_string()?,
                        &key,
                        self.position,
                    )?;
                }
                "oidc-audience" => {
                    set_once(
                        &mut oidc_audience,
                        self.optional_string()?,
                        &key,
                        self.position,
                    )?;
                }
                "oidc-client-id" => {
                    set_once(
                        &mut oidc_client_id,
                        self.optional_string()?,
                        &key,
                        self.position,
                    )?;
                }
                _ => return self.error(format!("unknown field {key}")),
            }
            self.expect_char(')')?;
        }
        self.skip_ignored();
        if self.position != self.source.len() {
            return self.error("unexpected trailing form");
        }
        Ok(SyncdConfig {
            schema: schema.context("xo-syncd configuration is missing schema")?,
            state_dir: state_dir.unwrap_or(None).map(PathBuf::from),
            bind: bind.unwrap_or(None),
            oidc_issuer: oidc_issuer.unwrap_or(None),
            oidc_audience: oidc_audience.unwrap_or(None),
            oidc_client_id: oidc_client_id.unwrap_or(None),
        })
    }

    fn integer(&mut self) -> Result<u16> {
        let value = self.token()?;
        value
            .parse()
            .with_context(|| format!("invalid unsigned schema at byte {}", self.position))
    }

    fn optional_string(&mut self) -> Result<Option<String>> {
        self.skip_ignored();
        if self.peek() == Some('"') {
            self.string().map(Some)
        } else if self.token()? == "#f" {
            Ok(None)
        } else {
            self.error("optional values must be a string or #f")
        }
    }

    fn string(&mut self) -> Result<String> {
        self.skip_ignored();
        let start = self.position;
        if self.bump() != Some('"') {
            return self.error("expected string");
        }
        let mut escaped = false;
        while let Some(value) = self.bump() {
            if escaped {
                escaped = false;
            } else if value == '\\' {
                escaped = true;
            } else if value == '"' {
                return serde_json::from_str(&self.source[start..self.position])
                    .context("invalid configuration string");
            }
        }
        self.error("unterminated string")
    }

    fn expect_token(&mut self, expected: &str) -> Result<()> {
        let actual = self.token()?;
        if actual == expected {
            Ok(())
        } else {
            self.error(format!("expected {expected}, found {actual}"))
        }
    }

    fn token(&mut self) -> Result<String> {
        self.skip_ignored();
        let start = self.position;
        while self
            .peek()
            .is_some_and(|value| !value.is_whitespace() && !matches!(value, '(' | ')' | ';' | '"'))
        {
            self.bump();
        }
        if self.position == start {
            self.error("expected token")
        } else {
            Ok(self.source[start..self.position].to_owned())
        }
    }

    fn expect_char(&mut self, expected: char) -> Result<()> {
        self.skip_ignored();
        if self.bump() == Some(expected) {
            Ok(())
        } else {
            self.error(format!("expected {expected:?}"))
        }
    }

    fn skip_ignored(&mut self) {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.bump();
            }
            if self.peek() != Some(';') {
                return;
            }
            while self.peek().is_some_and(|value| value != '\n') {
                self.bump();
            }
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.position..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let value = self.peek()?;
        self.position += value.len_utf8();
        Some(value)
    }

    fn error<T>(&self, message: impl std::fmt::Display) -> Result<T> {
        bail!(
            "invalid xo-syncd configuration at byte {}: {message}",
            self.position
        )
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, field: &str, position: usize) -> Result<()> {
    if slot.replace(value).is_some() {
        bail!("duplicate xo-syncd configuration field {field} at byte {position}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_round_trips_and_expands_home() {
        let config = SyncdConfig {
            schema: 1,
            state_dir: Some("~/.local/share/xo-syncd".into()),
            bind: Some("127.0.0.1:9464".into()),
            oidc_issuer: Some("https://id.example.test".into()),
            oidc_audience: Some("https://notes.example.test".into()),
            oidc_client_id: Some("xo-client".into()),
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.scm");
        std::fs::write(&path, config.document()).unwrap();
        let loaded =
            SyncdConfig::load_optional(&path, true, Some(Path::new("/home/test"))).unwrap();
        assert_eq!(
            loaded.state_dir,
            Some("/home/test/.local/share/xo-syncd".into())
        );
        assert_eq!(loaded.bind.as_deref(), Some("127.0.0.1:9464"));
        assert_eq!(loaded.oidc_client_id.as_deref(), Some("xo-client"));
    }

    #[test]
    fn rejects_unknown_duplicate_and_trailing_forms() {
        for source in [
            "(xo-syncd-config (schema 1) (unknown \"x\"))",
            "(xo-syncd-config (schema 1) (bind #f) (bind #f))",
            "(xo-syncd-config (schema 1)) (+ 1 2)",
        ] {
            assert!(Parser::new(source).parse().is_err(), "accepted {source}");
        }
    }

    #[test]
    fn missing_implicit_file_uses_empty_defaults() {
        let config = SyncdConfig::load_optional(
            Path::new("/definitely/missing/xo-syncd/config.scm"),
            false,
            None,
        )
        .unwrap();
        assert_eq!(config, SyncdConfig::default());
    }
}
