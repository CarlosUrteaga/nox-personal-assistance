use crate::config::{NewsSourceConfig, NewsSourceType};
use crate::tools::news::RawNewsItem;
use chrono::{DateTime, Utc};

pub async fn fetch_source_items(
    client: &reqwest::Client,
    source: &NewsSourceConfig,
) -> Result<Vec<RawNewsItem>, String> {
    match source.source_type {
        NewsSourceType::Rss => {
            let response = client
                .get(&source.url)
                .send()
                .await
                .map_err(|err| format!("Failed to fetch {}: {}", source.id, err))?;

            if !response.status().is_success() {
                return Err(format!(
                    "Failed to fetch {}: HTTP {}",
                    source.id,
                    response.status()
                ));
            }

            let body = response
                .text()
                .await
                .map_err(|err| format!("Failed to read feed body for {}: {}", source.id, err))?;

            parse_feed(&body, source)
        }
    }
}

pub fn parse_feed(body: &str, source: &NewsSourceConfig) -> Result<Vec<RawNewsItem>, String> {
    let mut items = parse_rss_items(body, source);
    if items.is_empty() {
        items = parse_atom_entries(body, source);
    }
    Ok(items)
}

fn parse_rss_items(body: &str, source: &NewsSourceConfig) -> Vec<RawNewsItem> {
    extract_blocks(body, "item")
        .into_iter()
        .filter_map(|block| {
            let title = first_tag_value(&block, &["title"])?;
            let link = first_tag_value(&block, &["link", "guid"]).unwrap_or_default();
            if link.trim().is_empty() {
                return None;
            }

            let snippet = first_tag_value(&block, &["description", "content:encoded"])
                .unwrap_or_default();
            let published_at = first_tag_value(&block, &["pubDate", "dc:date"])
                .and_then(|value| parse_timestamp(&value));

            Some(build_item(source, &title, &link, &snippet, published_at))
        })
        .collect()
}

fn parse_atom_entries(body: &str, source: &NewsSourceConfig) -> Vec<RawNewsItem> {
    extract_blocks(body, "entry")
        .into_iter()
        .filter_map(|block| {
            let title = first_tag_value(&block, &["title"])?;
            let link = atom_link(&block)?;
            let snippet = first_tag_value(&block, &["summary", "content"]).unwrap_or_default();
            let published_at = first_tag_value(&block, &["updated", "published"])
                .and_then(|value| parse_timestamp(&value));

            Some(build_item(source, &title, &link, &snippet, published_at))
        })
        .collect()
}

fn build_item(
    source: &NewsSourceConfig,
    title: &str,
    link: &str,
    snippet: &str,
    published_at: Option<DateTime<Utc>>,
) -> RawNewsItem {
    let clean_title = clean_text(title);
    let canonical_link = canonicalize_url(link);
    RawNewsItem {
        source_id: source.id.clone(),
        source_name: source.name.clone(),
        source_weight: source.source_weight,
        source_kind: source.source_kind.as_str().to_string(),
        normalized_title: normalize_title(&clean_title),
        title: clean_title,
        link: link.trim().to_string(),
        canonical_link,
        snippet: clean_text(snippet),
        published_at,
    }
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(value)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| DateTime::parse_from_rfc3339(value).map(|dt| dt.with_timezone(&Utc)))
        .ok()
}

fn extract_blocks(body: &str, tag: &str) -> Vec<String> {
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);
    let mut blocks = Vec::new();
    let mut rest = body;

    while let Some(start) = rest.find(&open) {
        let after_open = &rest[start..];
        let Some(open_end) = after_open.find('>') else {
            break;
        };
        let after_tag = &after_open[(open_end + 1)..];
        let Some(close_index) = after_tag.find(&close) else {
            break;
        };
        blocks.push(after_tag[..close_index].to_string());
        rest = &after_tag[(close_index + close.len())..];
    }

    blocks
}

fn first_tag_value(block: &str, tags: &[&str]) -> Option<String> {
    for tag in tags {
        if let Some(value) = tag_value(block, tag) {
            return Some(value);
        }
    }
    None
}

fn tag_value(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);
    let start = block.find(&open)?;
    let after_open = &block[start..];
    let open_end = after_open.find('>')?;
    let content = &after_open[(open_end + 1)..];
    let end = content.find(&close)?;
    Some(content[..end].to_string())
}

fn atom_link(block: &str) -> Option<String> {
    let link_tag = "<link";
    let start = block.find(link_tag)?;
    let rest = &block[start..];
    let end = rest.find('>')?;
    let tag = &rest[..end];
    if let Some(index) = tag.find("href=\"") {
        let value = &tag[(index + 6)..];
        let close = value.find('"')?;
        return Some(value[..close].to_string());
    }
    None
}

pub fn canonicalize_url(url: &str) -> String {
    let trimmed = url.trim();
    let without_fragment = trimmed.split('#').next().unwrap_or(trimmed);
    let (base, query) = without_fragment
        .split_once('?')
        .map(|(left, right)| (left, Some(right)))
        .unwrap_or((without_fragment, None));
    let normalized_base = base.trim_end_matches('/').replace("://m.", "://");

    let filtered_query = query
        .map(|value| {
            value
                .split('&')
                .filter(|part| {
                    let key = part.split('=').next().unwrap_or_default().to_ascii_lowercase();
                    !key.starts_with("utm_") && key != "fbclid" && key != "gclid"
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if filtered_query.is_empty() {
        normalized_base
    } else {
        format!("{}?{}", normalized_base, filtered_query.join("&"))
    }
}

fn normalize_title(value: &str) -> String {
    clean_text(value)
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch.is_ascii_whitespace() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn clean_text(value: &str) -> String {
    let without_cdata = value
        .replace("<![CDATA[", "")
        .replace("]]>", "")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">");

    let mut output = String::new();
    let mut inside_tag = false;
    for ch in without_cdata.chars() {
        match ch {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => output.push(ch),
            _ => {}
        }
    }

    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::{canonicalize_url, parse_feed};
    use crate::config::{NewsSourceConfig, NewsSourceKind, NewsSourceType};

    fn source() -> NewsSourceConfig {
        NewsSourceConfig {
            id: "source".to_string(),
            name: "Source".to_string(),
            source_type: NewsSourceType::Rss,
            url: "https://example.test/feed.xml".to_string(),
            enabled: true,
            source_weight: 0.8,
            source_kind: NewsSourceKind::Reporting,
        }
    }

    #[test]
    fn canonicalizes_tracking_params() {
        let url = canonicalize_url("https://example.test/path/?utm_source=x&id=1#frag");
        assert_eq!(url, "https://example.test/path?id=1");
    }

    #[test]
    fn parses_basic_rss_item() {
        let xml = r#"
            <rss><channel><item>
              <title>Agent orchestration update</title>
              <link>https://example.test/story?utm_source=x</link>
              <description><![CDATA[New retrieval orchestration release]]></description>
              <pubDate>Thu, 02 Apr 2026 15:00:00 GMT</pubDate>
            </item></channel></rss>
        "#;

        let items = parse_feed(xml, &source()).expect("parsed");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].canonical_link, "https://example.test/story");
        assert!(items[0].title.contains("Agent orchestration"));
    }
}
