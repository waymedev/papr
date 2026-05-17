//! Highlight export (feature F7). Turns an article and its highlights into a
//! Markdown document, or into request bodies for the Readwise and Notion APIs.
//!
//! The document/body *builders* are pure functions with unit tests below; the
//! actual network calls are the thin `post_to_readwise` / `post_to_notion`
//! functions, kept separate so the logic stays testable without a network.

use crate::error::{AppError, AppResult};
use crate::models::Highlight;
use reqwest::Client;
use serde_json::{json, Value};

/// The article fields an export needs. A small owned struct so the builders
/// stay pure — they never touch the database.
#[derive(Debug, Clone)]
pub struct ExportArticle {
    pub title: String,
    pub url: Option<String>,
    pub author: Option<String>,
    pub feed_title: String,
    pub published_at: Option<String>,
}

// ─────────────────────────── Markdown ───────────────────────────

/// Escape the characters that would otherwise be interpreted as Markdown
/// syntax inside a line of body text. Conservative — only the markers that
/// actually start inline constructs.
fn escape_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '|' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Render one highlight as a Markdown blockquote, with its note (if any) as a
/// nested italic line. The quote text is split on newlines so every line of a
/// multi-line quote keeps the `>` blockquote marker.
fn highlight_block(h: &Highlight) -> String {
    let mut block = String::new();
    for line in h.quote.split('\n') {
        block.push_str("> ");
        block.push_str(&escape_md(line));
        block.push('\n');
    }
    let note = h.note.trim();
    if !note.is_empty() {
        block.push_str(">\n");
        for line in note.split('\n') {
            block.push_str("> *");
            block.push_str(&escape_md(line));
            block.push_str("*\n");
        }
    }
    block
}

/// Build a complete Markdown document for an article and its highlights.
/// Suitable for an Obsidian vault note: a YAML-free header block, a source
/// link, then every highlight as a blockquote. Pure — fully unit-tested.
pub fn build_markdown(article: &ExportArticle, highlights: &[Highlight]) -> String {
    let mut doc = String::new();
    doc.push_str("# ");
    doc.push_str(&escape_md(&article.title));
    doc.push_str("\n\n");

    // Metadata lines — only the fields that are present.
    doc.push_str("- **Source:** ");
    doc.push_str(&escape_md(&article.feed_title));
    doc.push('\n');
    if let Some(author) = article.author.as_deref().filter(|a| !a.is_empty()) {
        doc.push_str("- **Author:** ");
        doc.push_str(&escape_md(author));
        doc.push('\n');
    }
    if let Some(date) = article.published_at.as_deref().filter(|d| !d.is_empty()) {
        doc.push_str("- **Published:** ");
        doc.push_str(&escape_md(date));
        doc.push('\n');
    }
    if let Some(url) = article.url.as_deref().filter(|u| !u.is_empty()) {
        // A bare link — URLs are not Markdown-escaped so they stay clickable.
        doc.push_str("- **Link:** ");
        doc.push_str(url);
        doc.push('\n');
    }
    doc.push('\n');

    doc.push_str("## Highlights\n\n");
    if highlights.is_empty() {
        doc.push_str("_No highlights yet._\n");
        return doc;
    }
    for (i, h) in highlights.iter().enumerate() {
        if i > 0 {
            doc.push('\n');
        }
        doc.push_str(&highlight_block(h));
    }
    doc
}

// ─────────────────────────── Readwise ───────────────────────────

/// Build the JSON body for a Readwise `POST /api/v2/highlights/` request.
/// Readwise accepts a batch under a `highlights` array; each entry carries the
/// quote plus the shared article metadata. Pure — unit-tested below.
pub fn build_readwise_body(article: &ExportArticle, highlights: &[Highlight]) -> Value {
    let items: Vec<Value> = highlights
        .iter()
        .map(|h| {
            let mut item = json!({
                "text": h.quote,
                "title": article.title,
                "source_type": "papr",
                "category": "articles",
            });
            if let Some(author) = article.author.as_deref().filter(|a| !a.is_empty()) {
                item["author"] = json!(author);
            }
            if let Some(url) = article.url.as_deref().filter(|u| !u.is_empty()) {
                item["source_url"] = json!(url);
            }
            let note = h.note.trim();
            if !note.is_empty() {
                item["note"] = json!(note);
            }
            item
        })
        .collect();
    json!({ "highlights": items })
}

/// POST a batch of highlights to Readwise. Thin wrapper over the pure builder.
pub async fn post_to_readwise(
    client: &Client,
    token: &str,
    article: &ExportArticle,
    highlights: &[Highlight],
) -> AppResult<()> {
    if highlights.is_empty() {
        return Err(AppError::code("noHighlights"));
    }
    let body = build_readwise_body(article, highlights);
    let resp = client
        .post("https://readwise.io/api/v2/highlights/")
        .header("Authorization", format!("Token {token}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(AppError::other(format!("Readwise error {status}: {detail}")));
    }
    Ok(())
}

// ─────────────────────────── Notion ───────────────────────────

/// A Notion rich-text run capped at Notion's 2000-character-per-run limit.
/// Longer text is split into multiple runs so the request is never rejected.
fn notion_rich_text(text: &str) -> Vec<Value> {
    if text.is_empty() {
        return vec![];
    }
    text.chars()
        .collect::<Vec<_>>()
        .chunks(2000)
        .map(|chunk| {
            let s: String = chunk.iter().collect();
            json!({ "type": "text", "text": { "content": s } })
        })
        .collect()
}

/// Build the JSON body for a Notion `PATCH /v1/blocks/{id}/children` request
/// that appends an article's highlights to a page. Each highlight becomes a
/// `quote` block; a note becomes a following italic paragraph. Pure.
pub fn build_notion_body(article: &ExportArticle, highlights: &[Highlight]) -> Value {
    let mut children: Vec<Value> = Vec::new();

    // A heading2 block introducing the article.
    children.push(json!({
        "object": "block",
        "type": "heading_2",
        "heading_2": { "rich_text": notion_rich_text(&article.title) },
    }));

    for h in highlights {
        children.push(json!({
            "object": "block",
            "type": "quote",
            "quote": { "rich_text": notion_rich_text(&h.quote) },
        }));
        let note = h.note.trim();
        if !note.is_empty() {
            children.push(json!({
                "object": "block",
                "type": "paragraph",
                "paragraph": {
                    "rich_text": [{
                        "type": "text",
                        "text": { "content": note },
                        "annotations": { "italic": true },
                    }],
                },
            }));
        }
    }
    json!({ "children": children })
}

/// Append an article's highlights to a Notion page as child blocks. Thin
/// wrapper over the pure builder.
pub async fn post_to_notion(
    client: &Client,
    token: &str,
    page_id: &str,
    article: &ExportArticle,
    highlights: &[Highlight],
) -> AppResult<()> {
    if highlights.is_empty() {
        return Err(AppError::code("noHighlights"));
    }
    let body = build_notion_body(article, highlights);
    let resp = client
        .patch(format!(
            "https://api.notion.com/v1/blocks/{page_id}/children"
        ))
        .header("Authorization", format!("Bearer {token}"))
        .header("Notion-Version", "2022-06-28")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(AppError::other(format!("Notion error {status}: {detail}")));
    }
    Ok(())
}

// ─────────── Notion — full-article page (feature F8) ───────────
//
// Distinct from `build_notion_body` above (F7), which *appends highlight
// blocks* to an existing page. The F8 "Send to…" action instead creates a
// whole new Notion page for the article, with its metadata as page properties
// and its text as child paragraph blocks.

/// Split a block of plain text into Notion `paragraph` blocks, one per
/// source line, each capped at Notion's 2000-char-per-run limit. Blank lines
/// are dropped. Pure.
fn notion_paragraphs(text: &str) -> Vec<Value> {
    text.split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            json!({
                "object": "block",
                "type": "paragraph",
                "paragraph": { "rich_text": notion_rich_text(line) },
            })
        })
        .collect()
}

/// Build the JSON body for a Notion `POST /v1/pages` request that creates a
/// new page for a whole article under `parent_page_id`. The article's plain
/// text is supplied already stripped of markup (`body_text`); each line
/// becomes a paragraph block. A leading bookmark block links back to the
/// source. Pure — unit-tested below.
pub fn build_notion_page(
    parent_page_id: &str,
    article: &ExportArticle,
    body_text: &str,
) -> Value {
    let mut children: Vec<Value> = Vec::new();

    // A source bookmark, when the article has a URL.
    if let Some(url) = article.url.as_deref().filter(|u| !u.is_empty()) {
        children.push(json!({
            "object": "block",
            "type": "bookmark",
            "bookmark": { "url": url },
        }));
    }
    // A metadata callout (feed · author · date) so the page is self-describing.
    let mut meta: Vec<String> = vec![format!("Source: {}", article.feed_title)];
    if let Some(author) = article.author.as_deref().filter(|a| !a.is_empty()) {
        meta.push(format!("By {author}"));
    }
    if let Some(date) = article.published_at.as_deref().filter(|d| !d.is_empty()) {
        meta.push(date.to_string());
    }
    children.push(json!({
        "object": "block",
        "type": "callout",
        "callout": {
            "rich_text": notion_rich_text(&meta.join("  ·  ")),
            "icon": { "type": "emoji", "emoji": "📰" },
        },
    }));

    // The article body — one paragraph block per non-empty line.
    children.extend(notion_paragraphs(body_text));

    json!({
        "parent": { "page_id": parent_page_id },
        "properties": {
            "title": {
                "title": notion_rich_text(&article.title),
            },
        },
        "children": children,
    })
}

/// Create a new Notion page for a whole article. Thin wrapper over the pure
/// `build_notion_page` builder; reuses the same Notion HTTP contract as
/// `post_to_notion`.
pub async fn post_article_to_notion(
    client: &Client,
    token: &str,
    parent_page_id: &str,
    article: &ExportArticle,
    body_text: &str,
) -> AppResult<()> {
    let body = build_notion_page(parent_page_id, article, body_text);
    let resp = client
        .post("https://api.notion.com/v1/pages")
        .header("Authorization", format!("Bearer {token}"))
        .header("Notion-Version", "2022-06-28")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(AppError::other(format!("Notion error {status}: {detail}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_article() -> ExportArticle {
        ExportArticle {
            title: "Rust in 2024".to_string(),
            url: Some("https://example.com/rust".to_string()),
            author: Some("Jane Doe".to_string()),
            feed_title: "Example Blog".to_string(),
            published_at: Some("2024-01-15".to_string()),
        }
    }

    fn hl(id: i64, quote: &str, note: &str) -> Highlight {
        Highlight {
            id,
            article_id: 1,
            quote: quote.to_string(),
            prefix: String::new(),
            suffix: String::new(),
            text_offset: 0,
            color: "yellow".to_string(),
            note: note.to_string(),
            created_at: "2024-01-15 10:00:00".to_string(),
        }
    }

    // ── Markdown ──

    #[test]
    fn markdown_includes_header_and_metadata() {
        let md = build_markdown(&sample_article(), &[hl(1, "borrow checker", "")]);
        assert!(md.starts_with("# Rust in 2024\n"));
        assert!(md.contains("- **Source:** Example Blog"));
        assert!(md.contains("- **Author:** Jane Doe"));
        assert!(md.contains("- **Published:** 2024-01-15"));
        assert!(md.contains("- **Link:** https://example.com/rust"));
        assert!(md.contains("> borrow checker"));
    }

    #[test]
    fn markdown_escapes_special_characters() {
        let mut a = sample_article();
        a.title = "C# *vs* _Rust_".to_string();
        let md = build_markdown(&a, &[hl(1, "a [link] and #hash", "")]);
        assert!(md.contains("# C\\# \\*vs\\* \\_Rust\\_"));
        assert!(md.contains("> a \\[link\\] and \\#hash"));
    }

    #[test]
    fn markdown_renders_note_as_nested_italic() {
        let md = build_markdown(&sample_article(), &[hl(1, "the quote", "my thought")]);
        assert!(md.contains("> the quote\n>\n> *my thought*\n"));
    }

    #[test]
    fn markdown_multiple_highlights_separated() {
        let md = build_markdown(
            &sample_article(),
            &[hl(1, "first", ""), hl(2, "second", "noted")],
        );
        assert!(md.contains("> first\n"));
        assert!(md.contains("> second\n"));
        assert!(md.contains("> *noted*"));
    }

    #[test]
    fn markdown_empty_highlight_list() {
        let md = build_markdown(&sample_article(), &[]);
        assert!(md.contains("## Highlights"));
        assert!(md.contains("_No highlights yet._"));
    }

    #[test]
    fn markdown_omits_absent_metadata() {
        let a = ExportArticle {
            title: "Untitled".to_string(),
            url: None,
            author: None,
            feed_title: "Feed".to_string(),
            published_at: None,
        };
        let md = build_markdown(&a, &[hl(1, "q", "")]);
        assert!(!md.contains("**Author:**"));
        assert!(!md.contains("**Link:**"));
        assert!(!md.contains("**Published:**"));
    }

    #[test]
    fn markdown_multiline_quote_keeps_blockquote_marker() {
        let md = build_markdown(&sample_article(), &[hl(1, "line one\nline two", "")]);
        assert!(md.contains("> line one\n> line two\n"));
    }

    // ── Readwise ──

    #[test]
    fn readwise_body_shape() {
        let body = build_readwise_body(&sample_article(), &[hl(1, "quote text", "a note")]);
        let items = body["highlights"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["text"], "quote text");
        assert_eq!(items[0]["title"], "Rust in 2024");
        assert_eq!(items[0]["author"], "Jane Doe");
        assert_eq!(items[0]["source_url"], "https://example.com/rust");
        assert_eq!(items[0]["note"], "a note");
        assert_eq!(items[0]["category"], "articles");
    }

    #[test]
    fn readwise_body_omits_empty_note() {
        let body = build_readwise_body(&sample_article(), &[hl(1, "q", "")]);
        assert!(body["highlights"][0].get("note").is_none());
    }

    #[test]
    fn readwise_body_special_characters_preserved() {
        let body = build_readwise_body(
            &sample_article(),
            &[hl(1, "quote with \"quotes\" & <tags>", "emoji 🎉")],
        );
        assert_eq!(
            body["highlights"][0]["text"],
            "quote with \"quotes\" & <tags>"
        );
        assert_eq!(body["highlights"][0]["note"], "emoji 🎉");
    }

    #[test]
    fn readwise_body_multiple_highlights() {
        let body = build_readwise_body(
            &sample_article(),
            &[hl(1, "a", ""), hl(2, "b", ""), hl(3, "c", "")],
        );
        assert_eq!(body["highlights"].as_array().unwrap().len(), 3);
    }

    // ── Notion ──

    #[test]
    fn notion_body_shape() {
        let body = build_notion_body(&sample_article(), &[hl(1, "the quote", "")]);
        let children = body["children"].as_array().unwrap();
        // heading + quote block.
        assert_eq!(children.len(), 2);
        assert_eq!(children[0]["type"], "heading_2");
        assert_eq!(children[1]["type"], "quote");
        assert_eq!(
            children[1]["quote"]["rich_text"][0]["text"]["content"],
            "the quote"
        );
    }

    #[test]
    fn notion_body_note_adds_paragraph() {
        let body = build_notion_body(&sample_article(), &[hl(1, "q", "my note")]);
        let children = body["children"].as_array().unwrap();
        // heading + quote + note paragraph.
        assert_eq!(children.len(), 3);
        assert_eq!(children[2]["type"], "paragraph");
        assert_eq!(
            children[2]["paragraph"]["rich_text"][0]["annotations"]["italic"],
            true
        );
    }

    #[test]
    fn notion_body_special_characters_preserved() {
        let body = build_notion_body(
            &sample_article(),
            &[hl(1, "quotes \" & <tags> 🎉", "")],
        );
        assert_eq!(
            body["children"][1]["quote"]["rich_text"][0]["text"]["content"],
            "quotes \" & <tags> 🎉"
        );
    }

    #[test]
    fn notion_rich_text_splits_long_runs() {
        let long = "x".repeat(5000);
        let runs = notion_rich_text(&long);
        // 5000 / 2000 → 3 runs (2000 + 2000 + 1000).
        assert_eq!(runs.len(), 3);
        let total: usize = runs
            .iter()
            .map(|r| r["text"]["content"].as_str().unwrap().chars().count())
            .sum();
        assert_eq!(total, 5000);
    }

    #[test]
    fn notion_body_empty_highlights_just_heading() {
        let body = build_notion_body(&sample_article(), &[]);
        assert_eq!(body["children"].as_array().unwrap().len(), 1);
    }

    // ── Notion — full-article page (F8) ──

    #[test]
    fn notion_page_shape() {
        let body = build_notion_page("PARENT", &sample_article(), "First line.\nSecond line.");
        assert_eq!(body["parent"]["page_id"], "PARENT");
        assert_eq!(
            body["properties"]["title"]["title"][0]["text"]["content"],
            "Rust in 2024"
        );
        let children = body["children"].as_array().unwrap();
        // bookmark + callout + 2 paragraphs.
        assert_eq!(children.len(), 4);
        assert_eq!(children[0]["type"], "bookmark");
        assert_eq!(children[0]["bookmark"]["url"], "https://example.com/rust");
        assert_eq!(children[1]["type"], "callout");
        assert_eq!(children[2]["type"], "paragraph");
        assert_eq!(
            children[2]["paragraph"]["rich_text"][0]["text"]["content"],
            "First line."
        );
        assert_eq!(
            children[3]["paragraph"]["rich_text"][0]["text"]["content"],
            "Second line."
        );
    }

    #[test]
    fn notion_page_callout_carries_metadata() {
        let body = build_notion_page("P", &sample_article(), "body");
        let callout = &body["children"][1]["callout"]["rich_text"][0]["text"]["content"];
        let text = callout.as_str().unwrap();
        assert!(text.contains("Source: Example Blog"));
        assert!(text.contains("By Jane Doe"));
        assert!(text.contains("2024-01-15"));
    }

    #[test]
    fn notion_page_omits_bookmark_when_no_url() {
        let a = ExportArticle {
            title: "Untitled".to_string(),
            url: None,
            author: None,
            feed_title: "Feed".to_string(),
            published_at: None,
        };
        let body = build_notion_page("P", &a, "only line");
        let children = body["children"].as_array().unwrap();
        // callout + 1 paragraph, no bookmark.
        assert_eq!(children.len(), 2);
        assert_eq!(children[0]["type"], "callout");
        // Callout has just the feed title — no "By"/date fragments.
        let meta = children[0]["callout"]["rich_text"][0]["text"]["content"]
            .as_str()
            .unwrap();
        assert_eq!(meta, "Source: Feed");
    }

    #[test]
    fn notion_page_empty_body_has_no_paragraphs() {
        let body = build_notion_page("P", &sample_article(), "   \n\n  ");
        let children = body["children"].as_array().unwrap();
        // Only bookmark + callout survive — blank lines are dropped.
        assert_eq!(children.len(), 2);
    }

    #[test]
    fn notion_page_special_characters_preserved() {
        let mut a = sample_article();
        a.title = "C# \" & <tags> 🎉".to_string();
        let body = build_notion_page("P", &a, "quotes \" & <tags> 🎉");
        assert_eq!(
            body["properties"]["title"]["title"][0]["text"]["content"],
            "C# \" & <tags> 🎉"
        );
        // body_text drops the bookmark+callout, paragraph is index 2.
        assert_eq!(
            body["children"][2]["paragraph"]["rich_text"][0]["text"]["content"],
            "quotes \" & <tags> 🎉"
        );
    }

    #[test]
    fn notion_page_splits_long_lines() {
        let long = "x".repeat(5000);
        let body = build_notion_page("P", &sample_article(), &long);
        // bookmark + callout + 1 paragraph; that paragraph's rich_text has 3 runs.
        let runs = body["children"][2]["paragraph"]["rich_text"]
            .as_array()
            .unwrap();
        assert_eq!(runs.len(), 3);
    }
}
