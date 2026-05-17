//! Feed parsing (RSS / Atom / JSON Feed via `feed-rs`), feed auto-discovery,
//! and source-type detection for the multi-source aggregation feature.

use crate::db::NewArticle;
use crate::error::AppResult;
use crate::models::{Enclosure, SourceType};
use crate::sanitize;
use feed_rs::model::{Entry, Feed as RawFeed};
use scraper::{Html, Selector};
use url::Url;

/// Metadata + articles extracted from a single feed document.
pub struct ParsedFeed {
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub articles: Vec<NewArticle>,
}

/// Parse raw feed bytes. `base_url` is the feed URL, used to resolve relatives.
pub fn parse_feed(bytes: &[u8], base_url: &str) -> AppResult<ParsedFeed> {
    let raw: RawFeed = feed_rs::parser::parse(bytes)?;

    let site_url = pick_site_url(&raw.links).or_else(|| Some(base_url.to_string()));
    let articles = raw
        .entries
        .iter()
        .filter_map(|e| map_entry(e, site_url.as_deref().unwrap_or(base_url)))
        .collect();

    Ok(ParsedFeed {
        title: raw.title.map(|t| t.content),
        site_url,
        description: raw.description.map(|t| t.content),
        icon: raw.icon.or(raw.logo).map(|i| i.uri),
        articles,
    })
}

fn pick_site_url(links: &[feed_rs::model::Link]) -> Option<String> {
    links
        .iter()
        .find(|l| l.rel.as_deref() == Some("alternate"))
        .or_else(|| links.iter().find(|l| l.rel.is_none()))
        .map(|l| l.href.clone())
}

fn map_entry(e: &Entry, base: &str) -> Option<NewArticle> {
    let url = pick_site_url(&e.links);
    let guid = if e.id.trim().is_empty() {
        url.clone()?
    } else {
        e.id.clone()
    };

    let raw_html = e
        .content
        .as_ref()
        .and_then(|c| c.body.clone())
        .or_else(|| e.summary.as_ref().map(|s| s.content.clone()))
        .unwrap_or_default();
    let content_html = if raw_html.is_empty() {
        None
    } else {
        Some(sanitize::sanitize(&raw_html, Some(base)))
    };
    let body_text = sanitize::html_to_text(&raw_html);

    let summary = e
        .summary
        .as_ref()
        .map(|s| sanitize::html_to_text(&s.content))
        .filter(|s| !s.is_empty());

    // Image: prefer a media thumbnail, then any image-typed media content.
    let image_url = e.media.iter().find_map(|m| {
        m.thumbnails
            .first()
            .map(|t| t.image.uri.clone())
            .or_else(|| {
                m.content.iter().find_map(|c| {
                    let is_img = c
                        .content_type
                        .as_ref()
                        .map(|t| t.ty().as_str() == "image")
                        .unwrap_or(false);
                    if is_img {
                        c.url.as_ref().map(|u| u.to_string())
                    } else {
                        None
                    }
                })
            })
    });

    // Enclosures: audio/video media content (podcasts, video).
    let enclosures: Vec<Enclosure> = e
        .media
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|c| {
            let url = c.url.as_ref()?.to_string();
            let mime_type = c.content_type.as_ref().map(|t| t.to_string());
            let is_av = c
                .content_type
                .as_ref()
                .map(|t| matches!(t.ty().as_str(), "audio" | "video"))
                .unwrap_or(false);
            if is_av {
                Some(Enclosure {
                    url,
                    mime_type,
                    length: c.size.map(|s| s as i64),
                })
            } else {
                None
            }
        })
        .collect();

    Some(NewArticle {
        guid,
        url,
        title: e
            .title
            .as_ref()
            .map(|t| t.content.clone())
            .unwrap_or_else(|| "(untitled)".into()),
        author: e.authors.first().map(|p| p.name.clone()),
        summary,
        content_html,
        body_text,
        image_url,
        published_at: e.published.or(e.updated).map(|d| d.to_rfc3339()),
        enclosures,
    })
}

/// Detect the source type from a feed/site URL — drives differentiated UI.
pub fn detect_source_type(url: &str) -> SourceType {
    let host = Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
        .unwrap_or_default();
    if host.contains("youtube.com") || host.contains("youtu.be") {
        SourceType::Youtube
    } else if host.contains("bsky.app") || host.contains("bsky.social") {
        SourceType::Bluesky
    } else {
        // Mastodon and podcast detection happen after parsing (see refine_source_type).
        SourceType::Rss
    }
}

/// Refine the source type once a feed has been parsed (e.g. audio enclosures
/// → podcast). `host` is the feed host for Mastodon's `/@user.rss` pattern.
pub fn refine_source_type(initial: SourceType, parsed: &ParsedFeed, feed_url: &str) -> SourceType {
    if initial != SourceType::Rss {
        return initial;
    }
    let has_audio = parsed.articles.iter().any(|a| {
        a.enclosures
            .iter()
            .any(|e| e.mime_type.as_deref().map(|m| m.starts_with("audio")).unwrap_or(false))
    });
    if has_audio {
        return SourceType::Podcast;
    }
    if feed_url.contains("/@") && feed_url.ends_with(".rss") {
        return SourceType::Mastodon;
    }
    SourceType::Rss
}

/// Given the HTML of a web page, find `<link rel="alternate">` feed URLs.
/// Runs synchronously (uses `scraper`); never hold the result across `.await`.
pub fn discover_feeds(html: &str, page_url: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    let selector = Selector::parse("link[rel~=alternate]").unwrap();
    let base = Url::parse(page_url).ok();
    let mut found = Vec::new();
    for el in doc.select(&selector) {
        let ty = el.value().attr("type").unwrap_or("").to_lowercase();
        let is_feed = ty.contains("rss") || ty.contains("atom") || ty.contains("json");
        if !is_feed {
            continue;
        }
        if let Some(href) = el.value().attr("href") {
            let resolved = base
                .as_ref()
                .and_then(|b| b.join(href).ok())
                .map(|u| u.to_string())
                .unwrap_or_else(|| href.to_string());
            if !found.contains(&resolved) {
                found.push(resolved);
            }
        }
    }
    found
}

/// Decide whether bytes look like a feed (vs an HTML page) by attempting a parse.
pub fn looks_like_feed(bytes: &[u8]) -> bool {
    feed_rs::parser::parse(bytes).is_ok()
}
