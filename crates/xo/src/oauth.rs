use std::collections::BTreeMap;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const AUTH_FILE: &str = ".config/xo/auth.json";
const DEFAULT_NATIVE_REDIRECT_URI: &str = "http://127.0.0.1:9465/callback";

#[derive(Debug, Deserialize)]
struct ServerConfig {
    #[serde(default)]
    disabled: bool,
    issuer: Option<String>,
    client_id: Option<String>,
    resource: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
    native_redirect_uri: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Discovery {
    authorization_endpoint: String,
    token_endpoint: String,
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
    let redirect_uri = native_redirect_uri(&config).to_owned();
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
    let token = authorize_code(
        &client,
        &discovery,
        &client_id,
        &resource,
        &config.scopes,
        &redirect_uri,
    )
    .await?;
    store.servers.insert(server.to_owned(), token.clone());
    save_store(&path, &store)?;
    Ok(Some(token.access_token))
}

async fn authorize_code(
    client: &reqwest::Client,
    discovery: &Discovery,
    client_id: &str,
    resource: &str,
    permissions: &[String],
    redirect_uri: &str,
) -> Result<SavedToken> {
    let redirect = url::Url::parse(redirect_uri).context("invalid native OIDC redirect URI")?;
    if redirect.scheme() != "http"
        || redirect.host_str() != Some("127.0.0.1")
        || redirect.port().is_none()
        || redirect.path() != "/callback"
        || redirect.query().is_some()
    {
        bail!("native OIDC redirect URI must be http://127.0.0.1:<port>/callback");
    }
    let listener =
        tokio::net::TcpListener::bind(("127.0.0.1", redirect.port().expect("port was checked")))
            .await
            .with_context(|| format!("bind OAuth callback listener at {redirect_uri}"))?;
    let verifier = random_base64_url(64);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()));
    let state = random_base64_url(32);
    let mut authorization = url::Url::parse(&discovery.authorization_endpoint)
        .context("invalid Pocket ID authorization endpoint")?;
    authorization
        .query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", &requested_scope(permissions))
        .append_pair("resource", resource)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);
    eprintln!("Open this URL to authorize xo:\n  {authorization}");
    if !open_browser(authorization.as_str()) {
        eprintln!("Could not open a browser automatically; open the URL above manually.");
    }
    let (code, returned_state) = tokio::time::timeout(
        Duration::from_secs(300),
        receive_callback(&listener, redirect.path()),
    )
    .await
    .context("Pocket ID browser authorization timed out")??;
    if returned_state != state {
        bail!("Pocket ID callback state does not match");
    }
    let response = client
        .post(&discovery.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier.as_str()),
            ("resource", resource),
        ])
        .send()
        .await
        .context("exchange Pocket ID authorization code")?;
    let status = response.status();
    let body = response.bytes().await?;
    let token: TokenResponse = serde_json::from_slice(&body)
        .with_context(|| format!("decode Pocket ID token response (HTTP {status})"))?;
    if !status.is_success() || token.error.is_some() {
        bail!(
            "Pocket ID token exchange failed: {}",
            token
                .error_description
                .as_deref()
                .or(token.error.as_deref())
                .unwrap_or("unknown error")
        );
    }
    saved_token(token, None)
}

async fn receive_callback(
    listener: &tokio::net::TcpListener,
    expected_path: &str,
) -> Result<(String, String)> {
    let (mut stream, _) = listener.accept().await?;
    let mut request = Vec::new();
    loop {
        let mut chunk = [0_u8; 1024];
        let count = stream.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if request.len() > 16 * 1024 {
            bail!("OAuth callback request is too large");
        }
    }
    let line = std::str::from_utf8(&request)?
        .lines()
        .next()
        .context("OAuth callback omitted request line")?;
    let target = line
        .split_ascii_whitespace()
        .nth(1)
        .context("OAuth callback omitted request target")?;
    let callback = url::Url::parse(&format!("http://127.0.0.1{target}"))?;
    if callback.path() != expected_path {
        bail!("OAuth callback used an unexpected path");
    }
    let parameters = callback.query_pairs().collect::<BTreeMap<_, _>>();
    let result = match parameters.get("error") {
        Some(error) => Err(anyhow::anyhow!(
            "Pocket ID authorization failed: {}",
            parameters
                .get("error_description")
                .map_or(error.as_ref(), std::borrow::Cow::as_ref)
        )),
        None => Ok((
            parameters
                .get("code")
                .context("OAuth callback omitted code")?
                .to_string(),
            parameters
                .get("state")
                .context("OAuth callback omitted state")?
                .to_string(),
        )),
    };
    let (status, message) = if result.is_ok() {
        ("200 OK", "xo is now authorized. You may close this window.")
    } else {
        (
            "400 Bad Request",
            "xo authorization failed. Return to the terminal for details.",
        )
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{message}",
        message.len()
    );
    stream.write_all(response.as_bytes()).await?;
    result
}

fn random_base64_url(length: usize) -> String {
    let mut bytes = vec![0_u8; length];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let command = Command::new("open").arg(url).status();
    #[cfg(target_os = "windows")]
    let command = Command::new("cmd").args(["/C", "start", "", url]).status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let command = Command::new("xdg-open").arg(url).status();
    command.is_ok_and(|status| status.success())
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

fn native_redirect_uri(config: &ServerConfig) -> &str {
    config
        .native_redirect_uri
        .as_deref()
        .unwrap_or(DEFAULT_NATIVE_REDIRECT_URI)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn loopback_callback_returns_code_and_state() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let callback =
            tokio::spawn(async move { receive_callback(&listener, "/callback").await.unwrap() });
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream
            .write_all(
                b"GET /callback?code=auth-code&state=expected HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            )
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(
            callback.await.unwrap(),
            ("auth-code".to_owned(), "expected".to_owned())
        );
    }

    #[test]
    fn older_server_config_uses_the_standard_loopback_callback() {
        let config: ServerConfig = serde_json::from_str(
            r#"{"issuer":"https://id.example.test","client_id":"xo","resource":"https://notes.example.test","scopes":[]}"#,
        )
        .unwrap();
        assert_eq!(native_redirect_uri(&config), DEFAULT_NATIVE_REDIRECT_URI);
    }

    #[test]
    fn requested_scopes_include_identity_refresh_and_api_permissions() {
        assert_eq!(
            requested_scope(&["xo:read".into(), "xo:write".into(), "xo:sync".into()]),
            "openid offline_access xo:read xo:write xo:sync"
        );
    }
}
