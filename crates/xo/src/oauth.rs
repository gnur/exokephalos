use std::collections::BTreeMap;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

const AUTH_FILE: &str = ".config/xo/auth.json";

#[derive(Debug, Deserialize)]
struct ServerConfig {
    #[serde(default)]
    disabled: bool,
    issuer: Option<String>,
    client_id: Option<String>,
    resource: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Discovery {
    device_authorization_endpoint: String,
    token_endpoint: String,
}

#[derive(Debug, Deserialize)]
struct DeviceAuthorization {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct SavedToken {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: u64,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct TokenStore {
    #[serde(default)]
    servers: BTreeMap<String, SavedToken>,
}

pub async fn access_token(server: &str, home: &Path) -> Result<Option<String>> {
    let client = reqwest::Client::new();
    let path = home.join(AUTH_FILE);
    let mut store = load_store(&path)?;
    let saved_fallback = store.servers.get(server).cloned();
    let config_url = format!(
        "{}/.well-known/xo-configuration",
        server.trim_end_matches('/')
    );
    let config: ServerConfig = match get_json(&client, &config_url).await {
        Ok(config) => config,
        Err(_) => return Ok(saved_fallback.map(|saved| saved.access_token)),
    };
    if config.disabled {
        return Ok(None);
    }
    let issuer = config.issuer.context("server omitted OIDC issuer")?;
    let client_id = config.client_id.context("server omitted OIDC client ID")?;
    let resource = config.resource.context("server omitted OIDC resource")?;
    let discovery: Discovery = match get_json(
        &client,
        &format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        ),
    )
    .await
    {
        Ok(discovery) => discovery,
        Err(_) => return Ok(saved_fallback.map(|saved| saved.access_token)),
    };
    if let Some(saved) = store.servers.get(server) {
        if saved.expires_at > unix_time().saturating_add(60) {
            return Ok(Some(saved.access_token.clone()));
        }
        if let Some(refresh_token) = &saved.refresh_token
            && let Ok(token) = refresh(
                &client,
                &discovery.token_endpoint,
                &client_id,
                &resource,
                &config.scopes,
                refresh_token,
            )
            .await
        {
            store.servers.insert(server.to_owned(), token.clone());
            save_store(&path, &store)?;
            return Ok(Some(token.access_token));
        }
    }
    if !std::io::stderr().is_terminal() {
        bail!("authentication is required; run xo interactively once to authorize this endpoint");
    }
    let token =
        device_authorize(&client, &discovery, &client_id, &resource, &config.scopes).await?;
    store.servers.insert(server.to_owned(), token.clone());
    save_store(&path, &store)?;
    Ok(Some(token.access_token))
}

async fn device_authorize(
    client: &reqwest::Client,
    discovery: &Discovery,
    client_id: &str,
    resource: &str,
    permissions: &[String],
) -> Result<SavedToken> {
    let scope = requested_scope(permissions);
    let response = client
        .post(&discovery.device_authorization_endpoint)
        .form(&[
            ("client_id", client_id),
            ("resource", resource),
            ("scope", &scope),
        ])
        .send()
        .await
        .context("start Pocket ID device authorization")?;
    let status = response.status();
    let body = response.bytes().await?;
    if !status.is_success() {
        bail!(
            "Pocket ID device authorization returned HTTP {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    let device: DeviceAuthorization = serde_json::from_slice(&body)?;
    eprintln!("Open this URL to authorize xo:");
    eprintln!(
        "  {}",
        device
            .verification_uri_complete
            .as_deref()
            .unwrap_or(&device.verification_uri)
    );
    eprintln!("Code: {}", device.user_code);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(device.expires_in);
    let mut interval = Duration::from_secs(device.interval.unwrap_or(5).max(1));
    loop {
        if tokio::time::Instant::now() >= deadline {
            bail!("Pocket ID device authorization expired");
        }
        tokio::time::sleep(interval).await;
        let response = client
            .post(&discovery.token_endpoint)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device.device_code.as_str()),
                ("client_id", client_id),
                ("resource", resource),
            ])
            .send()
            .await?;
        let token: TokenResponse = serde_json::from_slice(&response.bytes().await?)?;
        match token.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += Duration::from_secs(5),
            Some(error) => bail!(
                "Pocket ID authorization failed: {}",
                token.error_description.as_deref().unwrap_or(error)
            ),
            None => return saved_token(token, None),
        }
    }
}

async fn refresh(
    client: &reqwest::Client,
    endpoint: &str,
    client_id: &str,
    resource: &str,
    permissions: &[String],
    refresh_token: &str,
) -> Result<SavedToken> {
    let scope = requested_scope(permissions);
    let response = client
        .post(endpoint)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
            ("resource", resource),
            ("scope", scope.as_str()),
        ])
        .send()
        .await?;
    let status = response.status();
    let token: TokenResponse = serde_json::from_slice(&response.bytes().await?)?;
    if !status.is_success() || token.error.is_some() {
        bail!("Pocket ID token refresh failed");
    }
    saved_token(token, Some(refresh_token.to_owned()))
}

fn saved_token(token: TokenResponse, previous_refresh: Option<String>) -> Result<SavedToken> {
    Ok(SavedToken {
        access_token: token
            .access_token
            .context("Pocket ID omitted access token")?,
        refresh_token: token.refresh_token.or(previous_refresh),
        expires_at: unix_time().saturating_add(token.expires_in.unwrap_or(300)),
    })
}

fn requested_scope(permissions: &[String]) -> String {
    std::iter::once("openid")
        .chain(std::iter::once("offline_access"))
        .chain(permissions.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

async fn get_json<T: serde::de::DeserializeOwned>(
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
    serde_json::from_slice(&response.bytes().await?).with_context(|| format!("decode {url}"))
}

fn load_store(path: &Path) -> Result<TokenStore> {
    match std::fs::read(path) {
        Ok(value) => serde_json::from_slice(&value).context("decode saved xo authentication"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(TokenStore::default()),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn save_store(path: &Path, store: &TokenStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(store)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(temporary, path)?;
    Ok(())
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
