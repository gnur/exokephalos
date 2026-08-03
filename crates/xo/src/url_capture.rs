use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use dom_smoothie::Readability;
use quick_html2md::{MarkdownOptions, html_to_markdown_with_options};
use reqwest::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE, LOCATION, USER_AGENT};
use time::OffsetDateTime;
use url::Url;
use xo_core::domain::{Frontmatter, FrontmatterValue};
use xo_core::projection::canonical_note_path;
use xo_core::{Note, NoteId};

const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const USER_AGENT_VALUE: &str = "exokephalos-url-capture/1.0";
const EMPTY_CONTENT: &str = "(No readable content extracted.)";

pub type FetchFuture<'a> = Pin<Box<dyn Future<Output = Result<FetchedPage>> + Send + 'a>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchedPage {
    pub final_url: Url,
    pub content_type: String,
    pub body: Vec<u8>,
}

/// Test seam for network access. Steel receives no ambient network functions;
/// the application may invoke this service only after checking action grants.
pub trait PageFetcher: Send + Sync {
    fn fetch<'a>(&'a self, url: &'a Url) -> FetchFuture<'a>;
}

pub trait ReadableExtractor: Send + Sync {
    fn extract(&self, page: &FetchedPage) -> Result<CapturedPage>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedPage {
    pub url: Url,
    pub title: String,
    pub body: String,
    pub author: Option<String>,
    pub site: Option<String>,
    pub published: Option<String>,
    pub image: Option<String>,
    pub excerpt: Option<String>,
}

pub struct UrlCaptureService<F = NativePageFetcher, E = NativeReadableExtractor> {
    fetcher: F,
    extractor: E,
}

impl Default for UrlCaptureService {
    fn default() -> Self {
        Self {
            fetcher: NativePageFetcher,
            extractor: NativeReadableExtractor,
        }
    }
}

impl<F, E> UrlCaptureService<F, E>
where
    F: PageFetcher,
    E: ReadableExtractor,
{
    #[must_use]
    pub fn new(fetcher: F, extractor: E) -> Self {
        Self { fetcher, extractor }
    }

    pub async fn capture(&self, raw_url: &str) -> Result<CapturedPage> {
        let url = validated_url(raw_url)?;
        let page = self.fetcher.fetch(&url).await?;
        if !is_html(&page.content_type) {
            bail!("unsupported content type {:?}", page.content_type);
        }
        self.extractor.extract(&page)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativePageFetcher;

impl PageFetcher for NativePageFetcher {
    fn fetch<'a>(&'a self, url: &'a Url) -> FetchFuture<'a> {
        Box::pin(async move {
            let mut current = url.clone();
            for redirects in 0..=MAX_REDIRECTS {
                let response = pinned_client(&current)
                    .await?
                    .get(current.clone())
                    .header(USER_AGENT, USER_AGENT_VALUE)
                    .header(ACCEPT, "text/html,application/xhtml+xml")
                    .send()
                    .await
                    .with_context(|| format!("fetch {current}"))?;

                if response.status().is_redirection() {
                    if redirects == MAX_REDIRECTS {
                        bail!("too many redirects");
                    }
                    let location = response
                        .headers()
                        .get(LOCATION)
                        .context("redirect is missing Location")?
                        .to_str()
                        .context("redirect Location is not valid text")?;
                    current = current
                        .join(location)
                        .context("resolve redirect Location")?;
                    validate_url(&current)?;
                    continue;
                }
                if !response.status().is_success() {
                    bail!("fetch returned status {}", response.status());
                }
                if response.content_length().is_some_and(|size| {
                    size > u64::try_from(MAX_BODY_BYTES).expect("limit fits u64")
                }) {
                    bail!("response body exceeds {MAX_BODY_BYTES} bytes");
                }
                let content_type = response
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                let body = read_limited(response).await?;
                return Ok(FetchedPage {
                    final_url: current,
                    content_type,
                    body,
                });
            }
            unreachable!("redirect loop always returns or errors")
        })
    }
}

async fn pinned_client(url: &Url) -> Result<Client> {
    let host = url.host_str().context("URL has no host")?;
    let port = url.port_or_known_default().context("URL has no port")?;
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .with_context(|| format!("resolve {host}"))?
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        bail!("host resolved to no addresses");
    }
    for address in &addresses {
        if !is_public_ip(address.ip()) {
            bail!("refusing private or special address {}", address.ip());
        }
    }
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .resolve_to_addrs(host, &addresses)
        .build()
        .context("build URL capture HTTP client")
}

async fn read_limited(mut response: reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.context("read response body")? {
        if body.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
            bail!("response body exceeds {MAX_BODY_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeReadableExtractor;

impl ReadableExtractor for NativeReadableExtractor {
    fn extract(&self, page: &FetchedPage) -> Result<CapturedPage> {
        let html = std::str::from_utf8(&page.body).context("page is not UTF-8")?;
        let mut readability = Readability::new(html, Some(page.final_url.as_str()), None)
            .context("parse page document")?;
        let article = readability.parse().context("extract readable content")?;
        let options = MarkdownOptions::commonmark().base_url(page.final_url.as_str());
        let converted = html_to_markdown_with_options(article.content.as_ref(), &options);
        let readable = converted.trim();
        let body = if readable.is_empty() {
            let text = article.text_content.trim();
            if text.is_empty() { EMPTY_CONTENT } else { text }
        } else {
            readable
        };
        let title = nonempty(article.title)
            .unwrap_or_else(|| page.final_url.host_str().unwrap_or("Untitled").to_owned());
        Ok(CapturedPage {
            url: page.final_url.clone(),
            body: format!("# {title}\n\n{body}\n"),
            title,
            author: article.byline.and_then(nonempty),
            site: article.site_name.and_then(nonempty),
            published: article
                .published_time
                .and_then(nonempty)
                .map(normalize_published),
            image: article.image.and_then(nonempty),
            excerpt: article.excerpt.and_then(nonempty),
        })
    }
}

pub fn captured_note(page: CapturedPage, now: OffsetDateTime) -> Result<Note> {
    let id = NoteId::new(xo_core::id::generate(now));
    let mut frontmatter = Frontmatter::from([
        (
            "created".into(),
            FrontmatterValue::String(xo_core::timestamp::format(now)?),
        ),
        ("id".into(), FrontmatterValue::String(id.to_string())),
        ("source".into(), FrontmatterValue::String("url".into())),
        ("tags".into(), FrontmatterValue::Sequence(vec![])),
        ("title".into(), FrontmatterValue::String(page.title)),
        ("type".into(), FrontmatterValue::String("note".into())),
        ("url".into(), FrontmatterValue::String(page.url.to_string())),
    ]);
    insert_optional(&mut frontmatter, "author", page.author);
    insert_optional(&mut frontmatter, "site", page.site);
    insert_optional(&mut frontmatter, "published", page.published);
    insert_optional(&mut frontmatter, "image", page.image);
    insert_optional(&mut frontmatter, "excerpt", page.excerpt);
    Ok(Note {
        path: canonical_note_path(&id, &frontmatter),
        id,
        frontmatter,
        body: page.body,
    })
}

fn insert_optional(frontmatter: &mut Frontmatter, key: &str, value: Option<String>) {
    if let Some(value) = value.and_then(nonempty) {
        frontmatter.insert(key.to_owned(), FrontmatterValue::String(value));
    }
}

fn nonempty(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref().trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn normalize_published(value: String) -> String {
    use time::UtcOffset;
    use time::format_description::well_known::Rfc3339;

    OffsetDateTime::parse(&value, &Rfc3339).map_or(value, |instant| {
        let offset = UtcOffset::local_offset_at(instant).unwrap_or(instant.offset());
        xo_core::timestamp::format(instant.to_offset(offset))
            .unwrap_or_else(|_| instant.date().to_string())
    })
}

fn validated_url(raw_url: &str) -> Result<Url> {
    let url = Url::parse(raw_url.trim()).context("invalid URL")?;
    validate_url(&url)?;
    Ok(url)
}

fn validate_url(url: &Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("only http and https URLs are supported");
    }
    if url.host_str().is_none() {
        bail!("URL has no host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("URL credentials are not allowed");
    }
    Ok(())
}

fn is_html(content_type: &str) -> bool {
    let media_type = content_type.split(';').next().unwrap_or_default().trim();
    media_type.eq_ignore_ascii_case("text/html")
        || media_type.eq_ignore_ascii_case("application/xhtml+xml")
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

    #[derive(Clone)]
    struct FixtureFetcher(FetchedPage);

    impl PageFetcher for FixtureFetcher {
        fn fetch<'a>(&'a self, _url: &'a Url) -> FetchFuture<'a> {
            Box::pin(async { Ok(self.0.clone()) })
        }
    }

    #[tokio::test]
    async fn fixture_capture_extracts_readable_markdown_and_metadata() {
        let page = FetchedPage {
            final_url: Url::parse("https://example.com/posts/one").unwrap(),
            content_type: "text/html; charset=utf-8".into(),
            body: br#"<html><head>
                <title>Ignored navigation</title>
                <meta property="og:title" content="A useful article">
                <meta name="author" content="Ada">
                <meta property="og:site_name" content="Example">
                <meta property="article:published_time" content="2025-03-04">
                </head><body><nav>many unrelated links</nav><article>
                <h1>A useful article</h1>
                <p>This is a sufficiently substantial paragraph with readable content,
                useful detail, and enough words for article extraction.</p>
                <p><a href="/more">Read more</a></p>
                </article></body></html>"#
                .to_vec(),
        };
        let service = UrlCaptureService::new(FixtureFetcher(page), NativeReadableExtractor);
        let captured = service
            .capture("https://example.com/original")
            .await
            .unwrap();
        assert_eq!(captured.title, "A useful article");
        assert_eq!(captured.author.as_deref(), Some("Ada"));
        assert_eq!(captured.site.as_deref(), Some("Example"));
        assert!(captured.body.contains("sufficiently substantial"));
        assert!(captured.body.contains("https://example.com/more"));

        let instant = OffsetDateTime::from_unix_timestamp(1_741_046_400).unwrap();
        let note = captured_note(captured, instant).unwrap();
        assert!(xo_core::id::is_valid(note.id.as_str()));
        assert_eq!(
            note.frontmatter.get("source"),
            Some(&FrontmatterValue::String("url".into()))
        );
        assert_eq!(
            note.frontmatter.get("author"),
            Some(&FrontmatterValue::String("Ada".into()))
        );
        assert!(note.path.ends_with("-a-useful-article.md"));
    }

    #[tokio::test]
    async fn rejects_non_html_before_extraction() {
        let page = FetchedPage {
            final_url: Url::parse("https://example.com/file.pdf").unwrap(),
            content_type: "application/pdf".into(),
            body: vec![],
        };
        let service = UrlCaptureService::new(FixtureFetcher(page), NativeReadableExtractor);
        assert!(
            service
                .capture("https://example.com/file.pdf")
                .await
                .is_err()
        );
    }

    #[test]
    fn url_validation_and_network_boundaries_reject_unsafe_targets() {
        assert!(validated_url("file:///etc/passwd").is_err());
        assert!(validated_url("https://user:secret@example.com").is_err());
        let credential_redirect = Url::parse("https://user:secret@example.com").unwrap();
        assert!(validate_url(&credential_redirect).is_err());
        for address in [
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "192.0.0.1",
            "192.168.1.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
        ] {
            assert!(!is_public_ip(address.parse().unwrap()), "{address}");
        }
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }
}
