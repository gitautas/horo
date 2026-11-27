//! Lightweight OpenGraph / HTML title preview fetcher for pasted URLs.
//!
//! Only fetches `https://` HTML pages, caps reads at 64 KiB with a short
//! timeout, and never blocks the UI thread.

use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPreview {
    pub url: String,
    pub site: String,
    pub title: String,
    pub description: Option<String>,
}

/// Extract the first `https://` URL from a message body (if any), stripping
/// trailing sentence punctuation.
pub fn first_url(body: &str) -> Option<String> {
    for token in body.split_whitespace() {
        let trimmed = token.trim_matches(|c: char| matches!(c, '<' | '>' | '(' | ')' | '[' | ']' | '"' | '\''));
        if let Some(rest) = trimmed.strip_prefix("https://") {
            let cleaned = trimmed.trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | ':' | '!' | '?'));
            if rest.contains('.') && !cleaned.contains("matrix.to/") {
                return Some(cleaned.to_owned());
            }
        }
    }
    None
}

/// Fetch OpenGraph metadata for an `https://` URL. Returns `None` on network
/// errors, non-HTML responses, or pages without a usable title.
pub async fn fetch(url: &str) -> Option<LinkPreview> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    let host = parsed.host_str()?.to_owned();
    if host == "localhost" || host.ends_with(".local") {
        return None;
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("horo/0.12")
        .build()
        .ok()?;
    let mut resp = client.get(parsed).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let is_html = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.to_ascii_lowercase().contains("text/html"));
    if !is_html {
        return None;
    }

    const MAX_BYTES: usize = 64 * 1024;
    let mut buf = Vec::with_capacity(16 * 1024);
    while let Ok(Some(chunk)) = resp.chunk().await {
        let take = (MAX_BYTES - buf.len()).min(chunk.len());
        buf.extend_from_slice(&chunk[..take]);
        if buf.len() >= MAX_BYTES {
            break;
        }
    }
    let html = String::from_utf8_lossy(&buf);
    parse_html(url, &host, &html)
}

fn parse_html(url: &str, host: &str, html: &str) -> Option<LinkPreview> {
    let site = meta_content(html, "property", "og:site_name")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| host.strip_prefix("www.").unwrap_or(host).to_owned());
    let title = meta_content(html, "property", "og:title")
        .or_else(|| meta_content(html, "name", "twitter:title"))
        .or_else(|| tag_text(html, "title"))
        .map(|s| truncate(&s, 120))
        .filter(|s| !s.is_empty())?;
    let description = meta_content(html, "property", "og:description")
        .or_else(|| meta_content(html, "name", "description"))
        .map(|s| truncate(&s, 180))
        .filter(|s| !s.is_empty());
    Some(LinkPreview { url: url.to_owned(), site, title, description })
}

fn meta_content(html: &str, attr_key: &str, attr_val: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let needle = format!("{attr_key}=\"{attr_val}\"");
    let needle_sq = format!("{attr_key}='{attr_val}'");
    let mut offset = 0;
    while let Some(pos) = lower[offset..].find("<meta") {
        let start = offset + pos;
        let end = lower[start..].find('>')? + start;
        let tag_lower = &lower[start..=end];
        let tag_raw = &html[start..=end];
        if tag_lower.contains(&needle) || tag_lower.contains(&needle_sq) {
            if let Some(content) = extract_attr(tag_raw, tag_lower, "content") {
                return Some(decode_entities(content.trim()));
            }
        }
        offset = end + 1;
    }
    None
}

fn extract_attr<'a>(tag_raw: &'a str, tag_lower: &str, name: &str) -> Option<&'a str> {
    for quote in ['"', '\''] {
        let pat = format!("{name}={quote}");
        if let Some(i) = tag_lower.find(&pat) {
            let val_start = i + pat.len();
            if let Some(val_end) = tag_raw[val_start..].find(quote) {
                return Some(&tag_raw[val_start..val_start + val_end]);
            }
        }
    }
    None
}

fn tag_text(html: &str, tag: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = lower.find(&open)?;
    let content_start = lower[start..].find('>')? + start + 1;
    let content_end = lower[content_start..].find(&close)? + content_start;
    Some(decode_entities(html[content_start..content_end].trim()))
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() { format!("{head}…") } else { head }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_first_https_url_and_strips_punctuation() {
        assert_eq!(first_url("check out https://example.com/foo."), Some("https://example.com/foo".into()));
        assert_eq!(first_url("no link here"), None);
        assert_eq!(first_url("http://insecure.example.com"), None);
    }

    #[test]
    fn parses_opengraph_metadata() {
        let html = r#"<html><head>
            <meta property="og:site_name" content="GitHub">
            <meta property="og:title" content="horo &amp; friends">
            <meta property="og:description" content="A cute Matrix client">
        </head></html>"#;
        let preview = parse_html("https://github.com/gitautas/horo", "github.com", html).unwrap();
        assert_eq!(preview.site, "GitHub");
        assert_eq!(preview.title, "horo & friends");
        assert_eq!(preview.description.as_deref(), Some("A cute Matrix client"));
    }
}
