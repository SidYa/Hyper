//! Store-page details for an Epic game (description, publisher, release
//! date, tags, screenshots), like Heroic's `getExtraInfo`:
//!
//! 1. namespace → product slug through Epic's launcher GraphQL,
//! 2. `store-content.ak.epicgames.com/api/<lang>/content/products/<slug>`,
//!    whose `productHome` page carries the about text and the gallery.
//!
//! Results are cached per namespace + language for a week; failures are not
//! cached, so the next visit tries again.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::error::{Error, Result};
use crate::paths;
use crate::types::GameExtra;

const GRAPHQL: &str = "https://launcher.store.epicgames.com/graphql";
const CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// Store language for the UI language; anything Epic does not offer asks
/// for English. A page without a translation comes back in English anyway
/// (Limbo, Hades in `uk`), so a missing translation never fails.
fn store_lang(lang: &str) -> &'static str {
    match lang {
        "de" => "de",
        "es" => "es-ES",
        "fr" => "fr",
        "it" => "it",
        "ja" => "ja",
        "ko" => "ko",
        "pl" => "pl",
        "pt" | "pt_BR" => "pt-BR",
        "tr" => "tr",
        "uk" => "uk",
        "zh_Hans" => "zh-CN",
        _ => "en-US",
    }
}

fn cache_file(namespace: &str, lang: &str) -> PathBuf {
    let safe: String = namespace
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    paths::cache_dir()
        .join("epic-extra")
        .join(format!("{safe}-{lang}.json"))
}

fn read_cache(path: &PathBuf) -> Option<GameExtra> {
    let meta = std::fs::metadata(path).ok()?;
    let age = SystemTime::now()
        .duration_since(meta.modified().ok()?)
        .unwrap_or_default();
    if age > CACHE_TTL {
        return None;
    }
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn write_cache(path: &PathBuf, extra: &GameExtra) {
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_vec(extra)?)
    };
    if let Err(e) = write() {
        tracing::debug!(error = %e, "could not cache Epic store info");
    }
}

/// Same fallback Heroic uses when the slug lookup fails.
fn slug_from_title(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || *c == ' ')
        .collect::<String>()
        .replace(' ', "-")
}

async fn product_slug(http: &reqwest::Client, namespace: &str) -> Option<String> {
    // the namespace goes in as a variable, never spliced into the query text
    const QUERY: &str = r#"query getMappings($namespace: String!) { Catalog { catalogNs(namespace: $namespace) { mappings(pageType: "productHome") { pageSlug pageType } } } }"#;
    let v: Value = http
        .post(GRAPHQL)
        .json(&serde_json::json!({ "query": QUERY, "variables": { "namespace": namespace } }))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    v.pointer("/data/Catalog/catalogNs/mappings")?
        .as_array()?
        .iter()
        .find(|m| m.get("pageType").and_then(Value::as_str) == Some("productHome"))?
        .get("pageSlug")?
        .as_str()
        .map(str::to_owned)
}

/// "ACTION" / "open world" → "Action" / "Open World".
fn tag_label(tag: &str) -> String {
    tag.split([' ', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let lower = w.to_lowercase();
            let mut chars = lower.chars();
            chars
                .next()
                .map(|f| f.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pull the fields we show out of a store-content product document.
pub fn parse_store_content(v: &Value) -> Option<GameExtra> {
    let home = v
        .get("pages")?
        .as_array()?
        .iter()
        .find(|p| p.get("type").and_then(Value::as_str) == Some("productHome"))?
        .get("data")?;
    let text = |p: &str| {
        home.pointer(p)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };

    let mut screenshots: Vec<String> = Vec::new();
    let mut push = |url: Option<&str>| {
        if let Some(u) = url.filter(|u| u.starts_with("http"))
            && !screenshots.iter().any(|s| s == u)
        {
            screenshots.push(u.to_string());
        }
    };
    if let Some(items) = home.pointer("/carousel/items").and_then(Value::as_array) {
        for item in items {
            push(item.pointer("/image/src").and_then(Value::as_str));
        }
    }
    if let Some(images) = home
        .pointer("/gallery/galleryImages")
        .and_then(Value::as_array)
    {
        for img in images {
            push(img.get("src").and_then(Value::as_str));
        }
    }

    let tags = home
        .pointer("/meta/tags")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(tag_label).collect())
        .unwrap_or_default();

    Some(GameExtra {
        description: text("/about/description").or_else(|| text("/about/shortDescription")),
        developer: text("/about/developerAttribution"),
        publisher: text("/about/publisherAttribution"),
        release_date: text("/meta/releaseDate").map(|d| d.chars().take(10).collect()),
        tags,
        screenshots,
    })
}

pub async fn fetch(
    http: &reqwest::Client,
    namespace: &str,
    title: &str,
    lang: &str,
) -> Result<GameExtra> {
    let lang = store_lang(lang);
    let cache = cache_file(namespace, lang);
    if let Some(hit) = read_cache(&cache) {
        return Ok(hit);
    }
    let slug = product_slug(http, namespace)
        .await
        .unwrap_or_else(|| slug_from_title(title));
    let url = format!("https://store-content.ak.epicgames.com/api/{lang}/content/products/{slug}");
    let resp = http
        .get(&url)
        .send()
        .await
        .map_err(|e| Error::Other(format!("Epic store: {e}")))?;
    if !resp.status().is_success() {
        return Err(Error::Other(format!(
            "Epic store returned {} for {slug}",
            resp.status()
        )));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| Error::Other(format!("Epic store: {e}")))?;
    let extra =
        parse_store_content(&v).ok_or_else(|| Error::Other(format!("no store page for {slug}")))?;
    write_cache(&cache, &extra);
    Ok(extra)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_product_home() {
        let v = serde_json::json!({
            "pages": [
                { "type": "addon", "data": {} },
                { "type": "productHome", "data": {
                    "about": {
                        "description": "  Long text  ",
                        "shortDescription": "Short",
                        "developerAttribution": "Remedy",
                        "publisherAttribution": "505 Games"
                    },
                    "meta": { "releaseDate": "2019-08-27T14:00:00.000Z", "tags": ["ACTION", "OPEN_WORLD"] },
                    "carousel": { "items": [
                        { "image": { "src": "https://cdn2.unrealengine.com/a.jpg" } },
                        { "image": { } },
                        { "image": { "src": "https://cdn2.unrealengine.com/b.png" } }
                    ] },
                    "gallery": { "galleryImages": [
                        { "src": "https://cdn2.unrealengine.com/b.png" },
                        { "src": "https://cdn2.unrealengine.com/c.png" }
                    ] }
                } }
            ]
        });
        let e = parse_store_content(&v).unwrap();
        assert_eq!(e.description.as_deref(), Some("Long text"));
        assert_eq!(e.developer.as_deref(), Some("Remedy"));
        assert_eq!(e.publisher.as_deref(), Some("505 Games"));
        assert_eq!(e.release_date.as_deref(), Some("2019-08-27"));
        assert_eq!(e.tags, ["Action", "Open World"]);
        assert_eq!(
            e.screenshots,
            [
                "https://cdn2.unrealengine.com/a.jpg",
                "https://cdn2.unrealengine.com/b.png",
                "https://cdn2.unrealengine.com/c.png"
            ]
        );
    }

    #[test]
    fn languages_fall_back_to_english() {
        assert_eq!(store_lang("uk"), "uk");
        assert_eq!(store_lang("be"), "en-US");
        assert_eq!(store_lang("en"), "en-US");
        assert_eq!(store_lang("de"), "de");
    }
}
