use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use url::Url;
use xo_core::domain::Frontmatter;

#[derive(Debug, Deserialize, Serialize)]
struct ItemResponse {
    frontmatter: Frontmatter,
    body: Vec<String>,
}

/// Fetch one item from xo-syncd and render its API representation as YAML.
pub async fn fetch_yaml(server: &str, item_id: &str, access_token: Option<&str>) -> Result<String> {
    if !xo_core::id::is_valid(item_id) {
        bail!("invalid item ID {item_id:?}");
    }
    let endpoint = item_endpoint(server, item_id)?;
    let client = reqwest::Client::new();
    let mut request = client.get(endpoint.clone());
    if let Some(token) = access_token {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .with_context(|| format!("GET {endpoint}"))?;
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        bail!("GET {endpoint} failed with {status}: {detail}");
    }
    let item: ItemResponse = serde_json::from_slice(&response.bytes().await?)
        .with_context(|| format!("decode item response from {endpoint}"))?;
    serde_yaml::to_string(&item).context("encode item as YAML")
}

fn item_endpoint(server: &str, item_id: &str) -> Result<Url> {
    let mut endpoint = Url::parse(server).context("server must be an absolute HTTP(S) URL")?;
    match endpoint.scheme() {
        "http" | "https" => {}
        scheme => bail!("unsupported server URL scheme {scheme:?}"),
    }
    endpoint.set_path(&format!("/api/items/{item_id}"));
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    Ok(endpoint)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use xo_core::domain::FrontmatterValue;

    use super::{ItemResponse, item_endpoint};

    #[test]
    fn builds_the_item_api_endpoint() {
        assert_eq!(
            item_endpoint("https://notes.example.com/base?old=true", "abcdef2")
                .unwrap()
                .as_str(),
            "https://notes.example.com/api/items/abcdef2"
        );
    }

    #[test]
    fn serializes_the_api_shape_as_yaml() {
        let item = ItemResponse {
            frontmatter: BTreeMap::from([(
                "title".into(),
                FrontmatterValue::String("A note".into()),
            )]),
            body: vec!["First line\nSecond line".into()],
        };
        assert_eq!(
            serde_yaml::to_string(&item).unwrap(),
            "frontmatter:\n  title: A note\nbody:\n- |-\n  First line\n  Second line\n"
        );
    }
}
