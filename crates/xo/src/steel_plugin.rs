use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;
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

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PluginResult {
    pub choices: Vec<PluginChoice>,
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
}

pub async fn execute(
    source: String,
    entrypoint: String,
    input: String,
    capabilities: BTreeSet<Capability>,
) -> Result<PluginResult> {
    execute_with_host(
        source,
        entrypoint,
        input,
        capabilities,
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
    if source.len() > MAX_PLUGIN_BYTES {
        bail!("Steel plugin exceeds {MAX_PLUGIN_BYTES} bytes");
    }
    tokio::task::spawn_blocking(move || {
        execute_blocking(source, &entrypoint, input, capabilities, host)
    })
    .await
    .context("Steel plugin worker stopped")?
}

fn execute_blocking(
    source: String,
    entrypoint: &str,
    input: String,
    capabilities: BTreeSet<Capability>,
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
    engine.register_fn(
        "xo-http-post-json",
        move |url: String, headers: String, body: String| -> Result<String, String> {
            if !network_capabilities.contains(&Capability::Network) {
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
    serde_json::from_str(&output).context("decode Steel plugin result")
}

fn post_json(raw_url: &str, raw_headers: &str, body: &str) -> Result<String> {
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
            BTreeSet::new(),
        )
        .await
        .unwrap();
        assert_eq!(result.choices[0].label, "hello");
        assert_eq!(result.choices[0].note.body, "from Steel");

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

    struct HardcoverFixture;

    impl SteelHostServices for HardcoverFixture {
        fn read_secret(&self, name: &str) -> Result<String> {
            assert_eq!(name, "HARDCOVER_TOKEN");
            Ok("fixture-token".into())
        }

        fn post_json(&self, url: &str, headers: &str, body: &str) -> Result<String> {
            assert_eq!(url, "https://api.hardcover.app/v1/graphql");
            let headers: serde_json::Value = serde_json::from_str(headers)?;
            assert_eq!(headers["Authorization"], "Bearer fixture-token");
            let body: serde_json::Value = serde_json::from_str(body)?;
            assert_eq!(body["variables"]["query"], "Genesis");
            Ok(serde_json::json!({
                "data": { "search": { "results": [{
                    "title": "Genesis",
                    "author_names": ["Ken Lozito"],
                    "description": "Humanity's first colony.",
                    "pages": 328,
                    "image": "https://example.com/cover.jpg",
                    "external_ids": { "goodreads": "36284236-genesis" },
                    "isbn_13": "9781234567897",
                    "release_year": 2017,
                    "featured_series": {
                        "position": 1,
                        "series": { "name": "First Colony" }
                    }
                }] } }
            })
            .to_string())
        }
    }

    struct FailingHardcoverFixture;

    impl SteelHostServices for FailingHardcoverFixture {
        fn read_secret(&self, _name: &str) -> Result<String> {
            Ok("fixture-token".into())
        }

        fn post_json(&self, _url: &str, _headers: &str, _body: &str) -> Result<String> {
            bail!("fixture Hardcover outage")
        }
    }

    #[tokio::test]
    async fn hardcover_errors_are_returned_to_the_tui() {
        let error = execute_with_host(
            include_str!("../../../plugins/hardcover.scm").into(),
            "xo-plugin-run".into(),
            "Genesis".into(),
            BTreeSet::from([
                Capability::CreateNote,
                Capability::Network,
                Capability::ReadSecret,
            ]),
            Arc::new(FailingHardcoverFixture),
        )
        .await
        .unwrap_err();
        assert!(format!("{error:#}").contains("fixture Hardcover outage"));
    }

    #[tokio::test]
    async fn hardcover_search_and_normalization_execute_in_steel() {
        let result = execute_with_host(
            include_str!("../../../plugins/hardcover.scm").into(),
            "xo-plugin-run".into(),
            "Genesis".into(),
            BTreeSet::from([
                Capability::CreateNote,
                Capability::Network,
                Capability::ReadSecret,
            ]),
            Arc::new(HardcoverFixture),
        )
        .await
        .unwrap();
        assert_eq!(result.choices.len(), 1, "{result:#?}");
        assert_eq!(
            result.choices[0].label,
            "Genesis (First Colony, #1) — Ken Lozito"
        );
        assert_eq!(
            result.choices[0].note.frontmatter.get("tags"),
            Some(&xo_core::domain::FrontmatterValue::Sequence(vec![
                xo_core::domain::FrontmatterValue::String("to-read".into())
            ]))
        );
        assert_eq!(
            result.choices[0].note.frontmatter.get("pages"),
            Some(&xo_core::domain::FrontmatterValue::Integer(328))
        );
        assert_eq!(
            result.choices[0].note.frontmatter.get("url"),
            Some(&xo_core::domain::FrontmatterValue::String(
                "https://www.goodreads.com/book/show/36284236-genesis".into()
            ))
        );
        assert_eq!(result.choices[0].note.body, "Humanity's first colony.");
    }

    #[test]
    #[ignore = "requires HARDCOVER_TOKEN and the live Hardcover API"]
    fn hardcover_live_search_uses_configured_token() {
        assert!(
            std::env::var("HARDCOVER_TOKEN").is_ok_and(|token| !token.trim().is_empty()),
            "HARDCOVER_TOKEN must be set for the live integration test"
        );
        let result = execute_blocking(
            include_str!("../../../plugins/hardcover.scm").into(),
            "xo-plugin-run",
            "The Hobbit Tolkien".into(),
            BTreeSet::from([
                Capability::CreateNote,
                Capability::Network,
                Capability::ReadSecret,
            ]),
            Arc::new(NativeSteelHostServices),
        )
        .unwrap();
        assert!(
            !result.choices.is_empty(),
            "live Hardcover search was empty"
        );
        assert!(result.choices.iter().all(|choice| {
            choice.note.frontmatter.get("type")
                == Some(&xo_core::domain::FrontmatterValue::String("book".into()))
        }));
    }

    #[test]
    fn rejects_private_network_targets() {
        let error = post_json("https://127.0.0.1/graphql", "{}", "{}").unwrap_err();
        assert!(error.to_string().contains("public addresses"));
    }
}
