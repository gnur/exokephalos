use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

pub const READ_PERMISSION: &str = "xo:read";
pub const WRITE_PERMISSION: &str = "xo:write";
pub const SYNC_PERMISSION: &str = "xo:sync";

#[derive(Clone, Debug, Deserialize)]
struct Discovery {
    issuer: String,
    jwks_uri: String,
}

#[derive(Clone, Debug, Deserialize)]
struct Claims {
    #[allow(dead_code)]
    sub: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    permissions: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BrowserAuthConfig {
    pub issuer: String,
    pub client_id: String,
    pub resource: String,
    pub scopes: Vec<&'static str>,
}

#[derive(Debug)]
pub(crate) struct OidcVerifier {
    issuer: String,
    audience: String,
    jwks_uri: String,
    keys: RwLock<JwkSet>,
    client: reqwest::Client,
}

#[derive(Clone, Debug)]
pub enum Authenticator {
    Oidc {
        verifier: Arc<OidcVerifier>,
        browser: BrowserAuthConfig,
    },
    UnsafeDisabled,
}

impl Authenticator {
    pub async fn discover(issuer: &str, audience: &str, client_id: &str) -> Result<Self> {
        let issuer = issuer.trim_end_matches('/');
        let client = reqwest::Client::builder()
            .build()
            .context("build OIDC HTTP client")?;
        let discovery_url = format!("{issuer}/.well-known/openid-configuration");
        let discovery: Discovery = fetch_json(&client, &discovery_url).await?;
        if discovery.issuer.trim_end_matches('/') != issuer {
            bail!("OIDC discovery issuer does not match configured issuer");
        }
        let keys = fetch_json(&client, &discovery.jwks_uri).await?;
        Ok(Self::Oidc {
            verifier: Arc::new(OidcVerifier {
                issuer: discovery.issuer.clone(),
                audience: audience.trim_end_matches('/').to_owned(),
                jwks_uri: discovery.jwks_uri,
                keys: RwLock::new(keys),
                client,
            }),
            browser: BrowserAuthConfig {
                issuer: discovery.issuer,
                client_id: client_id.to_owned(),
                resource: audience.trim_end_matches('/').to_owned(),
                scopes: vec![READ_PERMISSION, WRITE_PERMISSION, SYNC_PERMISSION],
            },
        })
    }

    pub fn unsafe_disabled() -> Self {
        Self::UnsafeDisabled
    }

    #[cfg(test)]
    pub fn deny_for_tests() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Self::Oidc {
            verifier: Arc::new(OidcVerifier {
                issuer: "https://id.example.test".into(),
                audience: "https://notes.example.test".into(),
                jwks_uri: "https://id.example.test/jwks".into(),
                keys: RwLock::new(JwkSet { keys: Vec::new() }),
                client: reqwest::Client::new(),
            }),
            browser: BrowserAuthConfig {
                issuer: "https://id.example.test".into(),
                client_id: "xo".into(),
                resource: "https://notes.example.test".into(),
                scopes: vec![READ_PERMISSION, WRITE_PERMISSION, SYNC_PERMISSION],
            },
        }
    }

    pub fn browser_config(&self) -> Option<&BrowserAuthConfig> {
        match self {
            Self::Oidc { browser, .. } => Some(browser),
            Self::UnsafeDisabled => None,
        }
    }

    pub async fn authorize(&self, token: Option<&str>, permission: &str) -> Result<()> {
        let Self::Oidc { verifier, .. } = self else {
            return Ok(());
        };
        let token = token.context("missing bearer access token")?;
        verifier.verify(token, permission).await
    }
}

impl OidcVerifier {
    async fn verify(&self, token: &str, permission: &str) -> Result<()> {
        let header = decode_header(token).context("invalid access token header")?;
        if !matches!(
            header.alg,
            Algorithm::RS256
                | Algorithm::RS384
                | Algorithm::RS512
                | Algorithm::ES256
                | Algorithm::ES384
        ) {
            bail!("access token uses an unsupported signing algorithm");
        }
        if self
            .decode_with_keys(token, &header, permission)
            .await
            .is_ok()
        {
            return Ok(());
        }
        let refreshed = fetch_json(&self.client, &self.jwks_uri).await?;
        *self.keys.write().await = refreshed;
        self.decode_with_keys(token, &header, permission).await
    }

    async fn decode_with_keys(
        &self,
        token: &str,
        header: &jsonwebtoken::Header,
        permission: &str,
    ) -> Result<()> {
        let keys = self.keys.read().await;
        let jwk = header
            .kid
            .as_deref()
            .and_then(|kid| keys.find(kid))
            .context("access token signing key is unknown")?;
        let key = DecodingKey::from_jwk(jwk).context("unsupported OIDC signing key")?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.audience.as_str()]);
        let claims = decode::<Claims>(token, &key, &validation)
            .context("access token validation failed")?
            .claims;
        let mut permissions = claims
            .scope
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        permissions.extend(claims.permissions);
        if !permissions.contains(permission) {
            bail!("access token lacks {permission} permission");
        }
        Ok(())
    }
}

async fn fetch_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("request {url}"))?;
    if !response.status().is_success() {
        bail!("{url} returned HTTP {}", response.status());
    }
    let body = response.bytes().await.context("read OIDC response")?;
    serde_json::from_slice(&body).context("decode OIDC response")
}
