use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use steel::rvals::SteelVal;
use steel::steel_vm::engine::Engine;
use steel::steel_vm::interrupt::InterruptHandler;
use steel::steel_vm::register_fn::RegisterFn;
use url::Url;
use xo_core::behavior::Capability;
use xo_core::domain::Frontmatter;

const MAX_PLUGIN_BYTES: usize = 1_048_576;
const MAX_HTTP_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_PLUGIN_RESULT_BYTES: usize = 2 * 1024 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const VM_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Clone, Debug, Default, Serialize)]
pub struct PluginContext {
    pub selected_item_ids: Vec<String>,
    pub items: BTreeMap<String, PluginItem>,
    pub all_tags: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct PluginItem {
    pub frontmatter: Frontmatter,
    pub body: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PluginResult {
    pub choices: Vec<PluginChoice>,
    #[serde(default)]
    pub operations: Vec<PluginOperation>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "operation", rename_all = "kebab-case")]
pub enum PluginOperation {
    UpdateItem {
        id: String,
        frontmatter: Frontmatter,
        body: String,
    },
    CreateItem {
        frontmatter: Frontmatter,
        body: String,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PluginChoice {
    pub label: String,
    pub note: PluginNote,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PluginNote {
    #[serde(default)]
    pub frontmatter: Frontmatter,
    #[serde(default)]
    pub body: String,
}

pub trait SteelHostServices: Send + Sync {
    fn read_secret(&self, name: &str) -> Result<String>;
    fn post_json(&self, url: &str, headers: &str, body: &str) -> Result<String>;
    fn get(&self, _url: &str, _headers: &str) -> Result<String> {
        bail!("HTTP GET is unavailable for this Steel host")
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeSteelHostServices;

impl SteelHostServices for NativeSteelHostServices {
    fn read_secret(&self, name: &str) -> Result<String> {
        std::env::var(name).with_context(|| format!("secret {name} is unavailable"))
    }

    fn post_json(&self, url: &str, headers: &str, body: &str) -> Result<String> {
        post_json(url, headers, body)
    }

    fn get(&self, url: &str, headers: &str) -> Result<String> {
        get_http(url, headers)
    }
}

pub async fn execute(
    source: String,
    entrypoint: String,
    input: String,
    capabilities: BTreeSet<Capability>,
) -> Result<PluginResult> {
    execute_with_context(
        source,
        entrypoint,
        input,
        capabilities,
        PluginContext::default(),
    )
    .await
}

pub async fn execute_with_context(
    source: String,
    entrypoint: String,
    input: String,
    capabilities: BTreeSet<Capability>,
    context: PluginContext,
) -> Result<PluginResult> {
    execute_with_host_context(
        source,
        entrypoint,
        input,
        capabilities,
        context,
        Arc::new(NativeSteelHostServices),
    )
    .await
}

pub async fn execute_with_host(
    source: String,
    entrypoint: String,
    input: String,
    capabilities: BTreeSet<Capability>,
    host: Arc<dyn SteelHostServices>,
) -> Result<PluginResult> {
    execute_with_host_context(
        source,
        entrypoint,
        input,
        capabilities,
        PluginContext::default(),
        host,
    )
    .await
}

pub async fn execute_with_host_context(
    source: String,
    entrypoint: String,
    input: String,
    capabilities: BTreeSet<Capability>,
    context: PluginContext,
    host: Arc<dyn SteelHostServices>,
) -> Result<PluginResult> {
    if source.len() > MAX_PLUGIN_BYTES {
        bail!("Steel plugin exceeds {MAX_PLUGIN_BYTES} bytes");
    }
    tokio::task::spawn_blocking(move || {
        execute_blocking(source, &entrypoint, input, capabilities, context, host)
    })
    .await
    .context("Steel plugin worker stopped")?
}

#[allow(clippy::too_many_lines)]
fn execute_blocking(
    source: String,
    entrypoint: &str,
    input: String,
    capabilities: BTreeSet<Capability>,
    context: PluginContext,
    host: Arc<dyn SteelHostServices>,
) -> Result<PluginResult> {
    let mut engine = Engine::new_sandboxed();
    let secret_capabilities = capabilities.clone();
    let secret_host = Arc::clone(&host);
    engine.register_fn("xo-secret", move |name: String| -> Result<String, String> {
        if !secret_capabilities.contains(&Capability::ReadSecret) {
            return Err("read-secret capability denied".into());
        }
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err("secret name must contain only A-Z, 0-9, and _".into());
        }
        secret_host
            .read_secret(&name)
            .map_err(|error| format!("{error:#}"))
    });
    let network_capabilities = capabilities;
    let selected_ids = serde_json::to_string(&context.selected_item_ids)?;
    let all_tags = serde_json::to_string(&context.all_tags)?;
    let items = Arc::new(context.items);
    let operations = Arc::new(Mutex::new(Vec::<PluginOperation>::new()));
    engine.register_fn("xo-selected-item-ids", move || selected_ids.clone());
    engine.register_fn("xo-all-tags", move || all_tags.clone());
    let item_values = Arc::clone(&items);
    engine.register_fn(
        "xo-note-content",
        move |id: String| -> Result<String, String> {
            item_values.get(&id).map_or_else(
                || Err(format!("item {id} is unavailable")),
                |item| serde_json::to_string(item).map_err(|error| error.to_string()),
            )
        },
    );
    let update_operations = Arc::clone(&operations);
    let update_capabilities = network_capabilities.clone();
    engine.register_fn(
        "xo-update-items",
        move |encoded: String| -> Result<String, String> {
            if !update_capabilities.contains(&Capability::MutateNote) {
                return Err("mutate-note capability denied".into());
            }
            let values: Vec<PluginOperation> = serde_json::from_str(&encoded)
                .map_err(|error| format!("invalid update operations: {error}"))?;
            if values
                .iter()
                .any(|value| !matches!(value, PluginOperation::UpdateItem { .. }))
            {
                return Err("xo-update-items accepts only update-item operations".into());
            }
            update_operations
                .lock()
                .map_err(|_| "plugin operation lock poisoned".to_owned())?
                .extend(values);
            Ok("queued".into())
        },
    );
    let create_operations = Arc::clone(&operations);
    let create_capabilities = network_capabilities.clone();
    engine.register_fn(
        "xo-create-item",
        move |encoded: String| -> Result<String, String> {
            if !create_capabilities.contains(&Capability::CreateNote) {
                return Err("create-note capability denied".into());
            }
            let value: PluginOperation = serde_json::from_str(&encoded)
                .map_err(|error| format!("invalid create operation: {error}"))?;
            if !matches!(value, PluginOperation::CreateItem { .. }) {
                return Err("xo-create-item accepts only create-item operations".into());
            }
            create_operations
                .lock()
                .map_err(|_| "plugin operation lock poisoned".to_owned())?
                .push(value);
            Ok("queued".into())
        },
    );
    let get_capabilities = network_capabilities.clone();
    let get_host = Arc::clone(&host);
    engine.register_fn(
        "xo-http-get",
        move |url: String, headers: String| -> Result<String, String> {
            if !get_capabilities.contains(&Capability::Network) {
                return Err("network capability denied".into());
            }
            get_host
                .get(&url, &headers)
                .map_err(|error| format!("{error:#}"))
        },
    );
    let post_capabilities = network_capabilities.clone();
    engine.register_fn(
        "xo-http-post-json",
        move |url: String, headers: String, body: String| -> Result<String, String> {
            if !post_capabilities.contains(&Capability::Network) {
                return Err("network capability denied".into());
            }
            host.post_json(&url, &headers, &body)
                .map_err(|error| format!("{error:#}"))
        },
    );

    let interrupt = InterruptHandler::new(&mut engine, VM_TIMEOUT);
    let value = interrupt.run_with_timeout(|| {
        engine.run(source)?;
        engine.call_function_by_name_with_args(entrypoint, vec![SteelVal::StringV(input.into())])
    });
    let output = match value.context("Steel plugin execution failed")? {
        SteelVal::StringV(value) => value.to_string(),
        _ => bail!("Steel plugin entrypoint must return a JSON string"),
    };
    if output.len() > MAX_PLUGIN_RESULT_BYTES {
        bail!("Steel plugin result exceeds {MAX_PLUGIN_RESULT_BYTES} bytes");
    }
    let mut result: PluginResult =
        serde_json::from_str(&output).context("decode Steel plugin result")?;
    if !result.choices.is_empty() && !network_capabilities.contains(&Capability::CreateNote) {
        bail!("create-note capability denied for plugin choices");
    }
    let queued_operations = operations
        .lock()
        .map_err(|_| anyhow::anyhow!("plugin operation lock poisoned"))?
        .clone();
    result.operations.clone_from(&queued_operations);
    Ok(result)
}

fn get_http(raw_url: &str, raw_headers: &str) -> Result<String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let url = Url::parse(raw_url).context("parse plugin URL")?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        bail!("Steel plugin HTTP GET requires an HTTPS URL without credentials");
    }
    let host = url.host_str().context("plugin URL has no host")?.to_owned();
    let port = url
        .port_or_known_default()
        .context("plugin URL has no port")?;
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .context("resolve plugin URL")?
        .collect::<Vec<SocketAddr>>();
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        bail!("plugin URL must resolve only to public addresses");
    }
    let headers: BTreeMap<String, String> =
        serde_json::from_str(raw_headers).context("decode plugin HTTP headers")?;
    let mut header_map = HeaderMap::new();
    for (name, value) in headers {
        let lowercase = name.to_ascii_lowercase();
        if !matches!(lowercase.as_str(), "accept" | "user-agent") {
            bail!("plugin HTTP header {name} is not allowed");
        }
        header_map.insert(
            HeaderName::from_bytes(lowercase.as_bytes()).context("invalid HTTP header name")?,
            HeaderValue::from_str(&value).context("invalid HTTP header value")?,
        );
    }
    let client = Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve_to_addrs(&host, &addresses)
        .build()
        .context("build plugin HTTP client")?;
    let response = client
        .get(url)
        .headers(header_map)
        .send()
        .context("plugin HTTP GET")?;
    if !response.status().is_success() {
        bail!("plugin HTTP GET returned {}", response.status());
    }
    if response.content_length().is_some_and(|length| {
        length > u64::try_from(MAX_HTTP_BODY_BYTES).expect("body limit fits u64")
    }) {
        bail!("plugin HTTP response exceeds {MAX_HTTP_BODY_BYTES} bytes");
    }
    let mut bytes = Vec::new();
    response
        .take(u64::try_from(MAX_HTTP_BODY_BYTES + 1).expect("body limit fits u64"))
        .read_to_end(&mut bytes)
        .context("read plugin HTTP response")?;
    if bytes.len() > MAX_HTTP_BODY_BYTES {
        bail!("plugin HTTP response exceeds {MAX_HTTP_BODY_BYTES} bytes");
    }
    String::from_utf8(bytes).context("plugin HTTP response is not UTF-8")
}

fn post_json(raw_url: &str, raw_headers: &str, body: &str) -> Result<String> {
    // reqwest is built without an implicit crypto provider. Select ring before
    // constructing the capability-gated plugin client.
    let _ = rustls::crypto::ring::default_provider().install_default();
    if body.len() > MAX_HTTP_BODY_BYTES {
        bail!("HTTP request body exceeds {MAX_HTTP_BODY_BYTES} bytes");
    }
    let url = Url::parse(raw_url).context("parse plugin URL")?;
    if url.scheme() != "https" {
        bail!("Steel plugin HTTP requires HTTPS");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("URL credentials are not allowed");
    }
    let host = url.host_str().context("plugin URL has no host")?.to_owned();
    let port = url
        .port_or_known_default()
        .context("plugin URL has no port")?;
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .context("resolve plugin URL")?
        .collect::<Vec<SocketAddr>>();
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        bail!("plugin URL must resolve only to public addresses");
    }
    let headers: BTreeMap<String, String> =
        serde_json::from_str(raw_headers).context("decode plugin HTTP headers")?;
    let mut header_map = HeaderMap::new();
    for (name, value) in headers {
        let lowercase = name.to_ascii_lowercase();
        if !matches!(
            lowercase.as_str(),
            "accept" | "authorization" | "content-type" | "user-agent"
        ) {
            bail!("plugin HTTP header {name} is not allowed");
        }
        header_map.insert(
            HeaderName::from_bytes(lowercase.as_bytes()).context("invalid HTTP header name")?,
            HeaderValue::from_str(&value).context("invalid HTTP header value")?,
        );
    }
    let client = Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve_to_addrs(&host, &addresses)
        .build()
        .context("build plugin HTTP client")?;
    let response = client
        .post(url)
        .headers(header_map)
        .body(body.to_owned())
        .send()
        .context("plugin HTTP POST")?;
    if !response.status().is_success() {
        bail!("plugin HTTP POST returned {}", response.status());
    }
    if response.content_length().is_some_and(|length| {
        length > u64::try_from(MAX_HTTP_BODY_BYTES).expect("body limit fits u64")
    }) {
        bail!("plugin HTTP response exceeds {MAX_HTTP_BODY_BYTES} bytes");
    }
    let mut bytes = Vec::new();
    response
        .take(u64::try_from(MAX_HTTP_BODY_BYTES + 1).expect("body limit fits u64"))
        .read_to_end(&mut bytes)
        .context("read plugin HTTP response")?;
    if bytes.len() > MAX_HTTP_BODY_BYTES {
        bail!("plugin HTTP response exceeds {MAX_HTTP_BODY_BYTES} bytes");
    }
    String::from_utf8(bytes).context("plugin HTTP response is not UTF-8")
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (b == 18 || b == 19))
        || a >= 240)
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    let first = segments[0];
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
        || (first == 0x2001 && segments[1] == 0x0db8)
        || ip
            .to_ipv4_mapped()
            .is_some_and(|mapped| !is_public_ipv4(mapped)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn steel_can_read_native_selection_context() {
        let context = PluginContext {
            selected_item_ids: vec!["note001".into()],
            items: BTreeMap::from([(
                "note001".into(),
                PluginItem {
                    frontmatter: Frontmatter::from([(
                        "title".into(),
                        xo_core::domain::FrontmatterValue::String("One".into()),
                    )]),
                    body: "body".into(),
                },
            )]),
            all_tags: vec!["important".into()],
        };
        let source = r#"
            (define (xo-plugin-run input)
              (value->jsexpr-string
                (hash "choices"
                  (list (hash "label" (xo-selected-item-ids)
                              "note" (hash "frontmatter" (hash "type" "note")
                                           "body" (xo-note-content "note001")))))))
        "#;
        let result = execute_with_context(
            source.into(),
            "xo-plugin-run".into(),
            "ignored".into(),
            BTreeSet::from([Capability::CreateNote]),
            context,
        )
        .await;
        assert!(result.is_ok(), "context primitives failed: {result:?}");
    }

    #[tokio::test]
    async fn sandbox_runs_pure_steel_and_denies_ungranted_secrets() {
        let source = r#"
            (define (xo-plugin-run input)
              (value->jsexpr-string
                (hash "choices"
                  (list (hash "label" input
                              "note" (hash "frontmatter" (hash "type" "note")
                                           "body" "from Steel"))))))
        "#;
        let result = execute(
            source.into(),
            "xo-plugin-run".into(),
            "hello".into(),
            BTreeSet::from([Capability::CreateNote]),
        )
        .await
        .unwrap();
        assert_eq!(result.choices[0].label, "hello");
        assert_eq!(result.choices[0].note.body, "from Steel");

        let error = execute(
            source.into(),
            "xo-plugin-run".into(),
            "hello".into(),
            BTreeSet::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("create-note capability denied"));

        let error = execute(
            "(define (xo-plugin-run input) (xo-secret input))".into(),
            "xo-plugin-run".into(),
            "TOKEN".into(),
            BTreeSet::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("read-secret capability denied"));
    }

    struct GenericHostFixture;

    impl SteelHostServices for GenericHostFixture {
        fn read_secret(&self, name: &str) -> Result<String> {
            assert_eq!(name, "API_TOKEN");
            Ok("fixture-token".into())
        }

        fn post_json(&self, url: &str, headers: &str, body: &str) -> Result<String> {
            assert_eq!(url, "https://api.example.com/search");
            assert_eq!(headers, r#"{"Authorization":"fixture-token"}"#);
            assert_eq!(body, r#"{"query":"example"}"#);
            Ok(r#"{"title":"Example result"}"#.into())
        }
    }

    #[tokio::test]
    async fn capability_checked_host_services_are_available_to_plugins() {
        let source = r#"
            (define (xo-plugin-run input)
              (let* ([token (xo-secret "API_TOKEN")]
                     [headers (value->jsexpr-string (hash "Authorization" token))]
                     [body (value->jsexpr-string (hash "query" input))]
                     [response (string->jsexpr (xo-http-post-json
                                 "https://api.example.com/search" headers body))])
                (value->jsexpr-string
                  (hash "choices"
                    (list (hash "label" (hash-ref response 'title)
                                "note" (hash "frontmatter" (hash "type" "result")
                                             "body" "created by plugin")))))))
        "#;
        let result = execute_with_host(
            source.into(),
            "xo-plugin-run".into(),
            "example".into(),
            BTreeSet::from([
                Capability::CreateNote,
                Capability::Network,
                Capability::ReadSecret,
            ]),
            Arc::new(GenericHostFixture),
        )
        .await
        .unwrap();
        assert_eq!(result.choices[0].label, "Example result");
        assert_eq!(result.choices[0].note.body, "created by plugin");
    }

    #[test]
    fn rejects_private_network_targets() {
        let error = post_json("https://127.0.0.1/graphql", "{}", "{}").unwrap_err();
        assert!(error.to_string().contains("public addresses"));
    }
}
