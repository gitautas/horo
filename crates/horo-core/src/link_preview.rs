//! Lightweight OpenGraph / HTML embed preview & remote image fetcher for pasted URLs.
//!
//! Only fetches `https://` URLs, caps reads with short timeouts, and never
//! blocks the UI thread.

use std::time::Duration;

const USER_AGENT: &str = "Mozilla/5.0 (compatible; horo/0.12; +https://github.com/gitautas/horo)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPreview {
    pub url: String,
    pub site: String,
    pub title: String,
    pub description: Option<String>,
    pub image: Option<String>,
    /// Optional 0xRRGGBB accent color from `<meta name="theme-color">`.
    pub color: Option<u32>,
    /// True when the URL itself is a direct image (PNG/JPEG/GIF/WebP).
    pub is_direct_image: bool,
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

fn is_safe_url(parsed: &url::Url) -> bool {
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    !(host == "localhost" || host.ends_with(".local") || host == "127.0.0.1" || host == "::1")
}

fn build_client(timeout_secs: u64) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(USER_AGENT)
        .build()
        .ok()
}

#[derive(serde::Deserialize)]
struct OEmbedResponse {
    title: Option<String>,
    author_name: Option<String>,
    provider_name: Option<String>,
    thumbnail_url: Option<String>,
}

/// Known oEmbed endpoints for heavy sites whose HTML puts `<meta og:*>` far
/// down the document or behind consent walls (matching how Discord embeds them).
fn oembed_endpoint(parsed: &url::Url) -> Option<(url::Url, Option<u32>)> {
    let host = parsed.host_str()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let (base, color): (&str, Option<u32>) = match host {
        "youtube.com" | "m.youtube.com" | "music.youtube.com" | "youtu.be" => {
            ("https://www.youtube.com/oembed", Some(0xFF0000))
        }
        "vimeo.com" => ("https://vimeo.com/api/oembed.json", Some(0x1AB7EA)),
        "open.spotify.com" => ("https://open.spotify.com/oembed", Some(0x1DB954)),
        "soundcloud.com" => ("https://soundcloud.com/oembed", Some(0xFF5500)),
        "tiktok.com" | "vm.tiktok.com" => ("https://www.tiktok.com/oembed", None),
        "reddit.com" | "old.reddit.com" => ("https://www.reddit.com/oembed", Some(0xFF4500)),
        _ => return None,
    };
    let mut endpoint = url::Url::parse(base).ok()?;
    endpoint.query_pairs_mut().append_pair("url", parsed.as_str()).append_pair("format", "json");
    Some((endpoint, color))
}

async fn fetch_oembed(client: &reqwest::Client, original_url: &str, parsed: &url::Url, host: &str) -> Option<LinkPreview> {
    let (endpoint, color) = oembed_endpoint(parsed)?;
    let resp = client.get(endpoint).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let data: OEmbedResponse = resp.json().await.ok()?;
    let title = data.title.map(|s| truncate(s.trim(), 140)).filter(|s| !s.is_empty())?;
    let site = data
        .provider_name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| host.strip_prefix("www.").unwrap_or(host).to_owned());
    let description = data.author_name.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let image = data
        .thumbnail_url
        .and_then(|raw| url::Url::parse(raw.trim()).ok())
        .filter(is_safe_url)
        .map(|u| u.to_string());
    Some(LinkPreview {
        url: original_url.to_owned(),
        site,
        title,
        description,
        image,
        color,
        is_direct_image: false,
    })
}

/// Fetch OpenGraph metadata (or oEmbed / direct image info) for an `https://` URL.
pub async fn fetch(url: &str) -> Option<LinkPreview> {
    let parsed = url::Url::parse(url).ok()?;
    if !is_safe_url(&parsed) {
        return None;
    }
    let host = parsed.host_str()?.to_owned();
    let client = build_client(6)?;

    if let Some(preview) = fetch_oembed(&client, url, &parsed, &host).await {
        return Some(preview);
    }

    let mut resp = client
        .get(parsed.clone())
        .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml,image/*;q=0.9,*/*;q=0.8")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let final_url = resp.url().clone();
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if ct.starts_with("image/png")
        || ct.starts_with("image/jpeg")
        || ct.starts_with("image/gif")
        || ct.starts_with("image/webp")
    {
        return Some(LinkPreview {
            url: url.to_owned(),
            site: host.strip_prefix("www.").unwrap_or(&host).to_owned(),
            title: String::new(),
            description: None,
            image: Some(final_url.to_string()),
            color: None,
            is_direct_image: true,
        });
    }

    if !ct.contains("text/html") && !ct.contains("application/xhtml") {
        return None;
    }

    const MAX_BYTES: usize = 1024 * 1024;
    let mut buf = Vec::with_capacity(32 * 1024);
    while let Ok(Some(chunk)) = resp.chunk().await {
        let take = (MAX_BYTES - buf.len()).min(chunk.len());
        buf.extend_from_slice(&chunk[..take]);
        if buf.len() >= MAX_BYTES {
            break;
        }
        // Stop early once </head> has arrived — OpenGraph tags live in <head>.
        if buf.len() >= 1024 {
            let tail_start = buf.len().saturating_sub(chunk.len() + 16);
            if String::from_utf8_lossy(&buf[tail_start..]).to_ascii_lowercase().contains("</head>") {
                break;
            }
        }
    }
    let html = String::from_utf8_lossy(&buf);
    parse_html(url, &final_url, &host, &html)
}

/// Fetch a remote `https://` image (capped at 8 MiB) for embed thumbnails.
pub async fn fetch_image(url: &str) -> Option<(String, Vec<u8>)> {
    let parsed = url::Url::parse(url).ok()?;
    if !is_safe_url(&parsed) {
        return None;
    }
    let client = build_client(8)?;
    let mut resp = client
        .get(parsed)
        .header(reqwest::header::ACCEPT, "image/webp,image/png,image/jpeg,image/gif,*/*;q=0.5")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let mime = match ct.as_str() {
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" => ct,
        "image/jpg" => "image/jpeg".to_owned(),
        _ => return None,
    };

    const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
    if resp.content_length().is_some_and(|n| n as usize > MAX_IMAGE_BYTES) {
        return None;
    }
    let mut buf = Vec::with_capacity(64 * 1024);
    while let Ok(Some(chunk)) = resp.chunk().await {
        if buf.len() + chunk.len() > MAX_IMAGE_BYTES {
            return None;
        }
        buf.extend_from_slice(&chunk);
    }
    if buf.is_empty() {
        return None;
    }
    Some((mime, buf))
}

fn parse_html(url: &str, base: &url::Url, host: &str, html: &str) -> Option<LinkPreview> {
    let site = meta_any(html, "og:site_name")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| host.strip_prefix("www.").unwrap_or(host).to_owned());
    let title = meta_any(html, "og:title")
        .or_else(|| meta_any(html, "twitter:title"))
        .or_else(|| tag_text(html, "title"))
        .map(|s| truncate(&s, 140))
        .filter(|s| !s.is_empty())?;
    let description = meta_any(html, "og:description")
        .or_else(|| meta_any(html, "twitter:description"))
        .or_else(|| meta_content(html, "name", "description"))
        .map(|s| truncate(&s, 220))
        .filter(|s| !s.is_empty());
    let image = meta_any(html, "og:image:secure_url")
        .or_else(|| meta_any(html, "og:image"))
        .or_else(|| meta_any(html, "twitter:image:src"))
        .or_else(|| meta_any(html, "twitter:image"))
        .and_then(|raw| base.join(raw.trim()).ok())
        .filter(is_safe_url)
        .map(|u| u.to_string());
    let color = meta_content(html, "name", "theme-color").and_then(|raw| parse_hex_color(&raw));

    Some(LinkPreview {
        url: url.to_owned(),
        site,
        title,
        description,
        image,
        color,
        is_direct_image: false,
    })
}

fn meta_any(html: &str, key: &str) -> Option<String> {
    meta_content(html, "property", key).or_else(|| meta_content(html, "name", key))
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

fn parse_hex_color(raw: &str) -> Option<u32> {
    let hex = raw.trim().strip_prefix('#')?;
    match hex.len() {
        6 => u32::from_str_radix(hex, 16).ok(),
        3 => {
            let mut out = 0u32;
            for b in hex.bytes() {
                let d = (b as char).to_digit(16)?;
                out = (out << 8) | (d << 4) | d;
            }
            Some(out)
        }
        _ => None,
    }
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
    fn parses_opengraph_metadata_image_and_theme_color() {
        let html = r##"<html><head>
            <meta property="og:site_name" content="GitHub">
            <meta property="og:title" content="horo &amp; friends">
            <meta property="og:description" content="A cute Matrix client">
            <meta property="og:image" content="/assets/preview.png">
            <meta name="theme-color" content="#5865F2">
        </head></html>"##;
        let base = url::Url::parse("https://github.com/gitautas/horo").unwrap();
        let preview = parse_html("https://github.com/gitautas/horo", &base, "github.com", html).unwrap();
        assert_eq!(preview.site, "GitHub");
        assert_eq!(preview.title, "horo & friends");
        assert_eq!(preview.description.as_deref(), Some("A cute Matrix client"));
        assert_eq!(preview.image.as_deref(), Some("https://github.com/assets/preview.png"));
        assert_eq!(preview.color, Some(0x5865F2));
        assert!(!preview.is_direct_image);
    }

    #[test]
    fn builds_oembed_endpoint_for_youtube_and_spotify() {
        let yt = url::Url::parse("https://www.youtube.com/watch?v=9j4HJp-AsOo").unwrap();
        let (ep, color) = oembed_endpoint(&yt).unwrap();
        assert_eq!(ep.host_str(), Some("www.youtube.com"));
        assert_eq!(ep.path(), "/oembed");
        assert_eq!(color, Some(0xFF0000));

        let sp = url::Url::parse("https://open.spotify.com/track/123").unwrap();
        let (ep, color) = oembed_endpoint(&sp).unwrap();
        assert_eq!(ep.host_str(), Some("open.spotify.com"));
        assert_eq!(color, Some(0x1DB954));

        let other = url::Url::parse("https://example.com/page").unwrap();
        assert!(oembed_endpoint(&other).is_none());
    }
}
