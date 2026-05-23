//! Readwise Reader API client — read-only document pull (feature CWM-37).
//!
//! Step 2 of the Reader integration. The data foundation (a single synthetic
//! `feeds` row and the `readwise_documents` side-table) was laid in step 1.
//! This module is the transport + mapping layer: it pulls `GET
//! https://readwise.io/api/v3/list/` with cursor pagination, a 20-req/min
//! throttle and `Retry-After`-aware 429 back-off, then maps each Reader
//! document to a [`NewArticle`] ready for the existing upsert path. The
//! persistence, refresh plumbing and Tauri command live in later steps.
//!
//! The module is structured for testability without a live HTTP server: the
//! pagination loop runs against a [`PageTransport`] trait, so unit tests can
//! script cursor / 429 sequences with a vector-backed mock. Mapping, the
//! parent-document filter, and `Retry-After` parsing are pure functions and
//! tested directly.
//!
//! Step 3 wires this module into the Tauri command surface
//! (`readwise_reader_sync`) and the matching DB upsert
//! (`db::upsert_readwise_document`); the front-end controls land in step 4.
//! A few items remain ahead of their final callers (the public `read_token`
//! helper is exercised through the command's settings path, not directly
//! here), so the dead-code lint is suppressed at the module boundary rather
//! than scattering `#[allow]` over every item.
#![allow(dead_code)]

use crate::db::{self, NewArticle, NewReaderDocument};
use crate::error::{AppError, AppResult};
use crate::sanitize;
use reqwest::{Client, StatusCode};
use rusqlite::Connection;
use serde::Deserialize;
use std::time::{Duration, Instant};

/// Reader API endpoint. Parameterised over `base_url` in the transport so
/// tests can point at a local mock, but production always hits this URL.
const READER_LIST_URL: &str = "https://readwise.io/api/v3/list/";

/// Readwise documents 20 req/min for the Reader API (60s / 20 = 3000ms).
/// Applied between successive page requests within one `fetch_documents`
/// call so a multi-page pull stays under the documented limit even when the
/// server is fast enough that we'd otherwise hammer it back-to-back.
const DEFAULT_MIN_INTERVAL: Duration = Duration::from_millis(3_000);

/// Cap on automatic retries for a single page when the server keeps replying
/// 429 — bounded so a degenerate (or hostile) server can't loop us forever.
const MAX_RETRIES: u32 = 5;

/// Hard ceiling on any single `Retry-After` wait. A misconfigured server
/// returning `Retry-After: 3600` would otherwise pin a sync for an hour.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(120);

/// Setting key shared with the existing Readwise highlights integration —
/// the token is stored once and reused here so the user doesn't paste it
/// twice. The token is never logged.
pub const TOKEN_SETTING: &str = "readwise_token";

/// Query knobs for one document-list pull. Defaults are "all updated docs,
/// no html content"; callers fill in the slice they want.
#[derive(Debug, Default, Clone)]
pub struct FetchOptions {
    /// Only documents updated after this RFC3339 timestamp.
    pub updated_after: Option<String>,
    /// One of `new` / `later` / `shortlist` / `archive` / `feed`.
    pub location: Option<String>,
    /// One of `article` / `email` / `rss` / `highlight` / `note` / `pdf`
    /// / `epub` / `tweet` / `video`.
    pub category: Option<String>,
    /// When true, ask the server for the `html_content` field. The Reader
    /// API only emits it on request because the payload doubles or triples
    /// in size.
    pub with_html_content: bool,
}

/// One row from the Reader list endpoint. Only the fields Papr consumes are
/// decoded; the rest are dropped. `parent_id` is decoded so we can filter
/// out child rows (highlights / notes that hang off a parent document) —
/// only parents become articles.
#[derive(Debug, Clone, Deserialize)]
pub struct ReaderDocument {
    pub id: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub image_url: Option<String>,
    /// Only populated when the request set `withHtmlContent=true`.
    #[serde(default)]
    pub html_content: Option<String>,
    #[serde(default)]
    pub published_date: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub reading_progress: Option<f64>,
}

impl ReaderDocument {
    /// A child document (highlight / note) hangs off a parent and is not an
    /// article in Papr's model — `upsert_article` would treat it as a
    /// stand-alone row. Filter these out at the boundary.
    pub fn is_subdocument(&self) -> bool {
        self.parent_id.as_deref().is_some_and(|p| !p.is_empty())
    }
}

/// Decoded `GET /api/v3/list/` response, restricted to the fields the
/// pagination loop consumes.
#[derive(Debug, Clone, Deserialize)]
struct ListResponse {
    #[serde(default)]
    results: Vec<ReaderDocument>,
    /// The Reader API returns this in camelCase even though the result rows
    /// use snake_case. Decode both spellings so a server-side rename does
    /// not silently end pagination at page 1.
    #[serde(default, alias = "next_page_cursor", rename = "nextPageCursor")]
    next_page_cursor: Option<String>,
}

/// Outcome of one page request. `RateLimited` carries the suggested wait so
/// the caller can sleep before retrying without re-parsing the response.
#[derive(Debug)]
enum PageOutcome {
    Page {
        results: Vec<ReaderDocument>,
        next_cursor: Option<String>,
    },
    RateLimited(Duration),
}

/// Abstracts a single page request so the pagination/retry loop can be
/// unit-tested with a scripted mock instead of a real HTTP server.
trait PageTransport {
    async fn fetch(&mut self, cursor: Option<&str>) -> AppResult<PageOutcome>;
}

/// Production [`PageTransport`] backed by a shared `reqwest::Client`.
///
/// Enforces the 20-req/min throttle by sleeping for the unspent remainder
/// of `DEFAULT_MIN_INTERVAL` before each call. Storing only the last
/// timestamp (vs. a sliding window) is enough because we only ever fire
/// from this single loop — there is no concurrent caller within one
/// `fetch_documents` invocation.
struct ReqwestTransport<'a> {
    http: &'a Client,
    token: &'a str,
    opts: &'a FetchOptions,
    base_url: &'a str,
    last_request: Option<Instant>,
}

impl<'a> PageTransport for ReqwestTransport<'a> {
    async fn fetch(&mut self, cursor: Option<&str>) -> AppResult<PageOutcome> {
        if let Some(prev) = self.last_request {
            let elapsed = prev.elapsed();
            if elapsed < DEFAULT_MIN_INTERVAL {
                tokio::time::sleep(DEFAULT_MIN_INTERVAL - elapsed).await;
            }
        }
        self.last_request = Some(Instant::now());

        let query = build_query(self.opts, cursor);
        let resp = self
            .http
            .get(self.base_url)
            .header("Authorization", format!("Token {}", self.token))
            .query(&query)
            .send()
            .await?;

        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            let wait = parse_retry_after(
                resp.headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok()),
            );
            return Ok(PageOutcome::RateLimited(wait));
        }
        let resp = resp.error_for_status()?;
        let body: ListResponse = resp.json().await?;
        Ok(PageOutcome::Page {
            results: body.results,
            next_cursor: body.next_page_cursor,
        })
    }
}

/// Build the query-string parameters for one page request. Empty / `None`
/// knobs are omitted so the server uses its default. Extracted into a pure
/// function so its shape can be asserted without firing a request.
fn build_query(opts: &FetchOptions, cursor: Option<&str>) -> Vec<(&'static str, String)> {
    let mut q: Vec<(&'static str, String)> = Vec::new();
    if let Some(v) = &opts.updated_after {
        q.push(("updatedAfter", v.clone()));
    }
    if let Some(v) = &opts.location {
        q.push(("location", v.clone()));
    }
    if let Some(v) = &opts.category {
        q.push(("category", v.clone()));
    }
    if opts.with_html_content {
        q.push(("withHtmlContent", "true".to_string()));
    }
    if let Some(c) = cursor {
        q.push(("pageCursor", c.to_string()));
    }
    q
}

/// Parse the `Retry-After` header into a wait duration. Readwise (like most
/// JSON APIs) only emits the integer-seconds form; the HTTP-date form is
/// not handled — if we ever see one we fall back to the default interval
/// rather than panic.
fn parse_retry_after(header: Option<&str>) -> Duration {
    header
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_MIN_INTERVAL)
}

/// Read the stored Readwise API token. Empty strings (which the settings
/// table uses as a tombstone for "cleared") return `None` so callers can
/// short-circuit with `noReadwiseToken` instead of issuing an unauthenticated
/// request.
pub fn read_token(conn: &Connection) -> AppResult<Option<String>> {
    Ok(db::get_setting(conn, TOKEN_SETTING)?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty()))
}

/// Drive the pagination + 429 retry loop against an arbitrary transport.
/// Generic over `T` so production fires real HTTP and tests script a Vec
/// of `PageOutcome`s. The loop:
/// - chains pages by `nextPageCursor` until the server returns `None`;
/// - on `RateLimited(wait)` sleeps `min(wait, MAX_RETRY_AFTER)` and retries
///   the **same** cursor up to `MAX_RETRIES` times before giving up.
async fn paginate<T: PageTransport>(transport: &mut T) -> AppResult<Vec<ReaderDocument>> {
    let mut out: Vec<ReaderDocument> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut attempts = 0u32;
        let (results, next_cursor) = loop {
            match transport.fetch(cursor.as_deref()).await? {
                PageOutcome::Page {
                    results,
                    next_cursor,
                } => break (results, next_cursor),
                PageOutcome::RateLimited(wait) => {
                    attempts += 1;
                    if attempts > MAX_RETRIES {
                        return Err(AppError::code("readwiseRateLimited"));
                    }
                    tokio::time::sleep(wait.min(MAX_RETRY_AFTER)).await;
                }
            }
        };
        out.extend(results);
        cursor = next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok(out)
}

/// Pull every parent document matching `opts`, paginated and rate-limited.
/// Sub-documents (`parent_id` non-null — highlights, notes) are dropped at
/// the boundary so callers only ever see article-shaped rows.
pub async fn fetch_documents(
    http: &Client,
    token: &str,
    opts: &FetchOptions,
) -> AppResult<Vec<ReaderDocument>> {
    let mut transport = ReqwestTransport {
        http,
        token,
        opts,
        base_url: READER_LIST_URL,
        last_request: None,
    };
    let docs = paginate(&mut transport).await?;
    Ok(docs.into_iter().filter(|d| !d.is_subdocument()).collect())
}

/// Map a Reader document onto the existing `NewArticle` shape so the
/// rest of the ingestion pipeline (rules, dedup, FTS, retention) needs no
/// special-casing. Persistence and the side-table row land in the next step.
///
/// Field mapping:
/// - `guid`         = `document.id` (Reader-side opaque id, globally unique)
/// - `url`          = `source_url ?? url` (canonical web URL; falls back to
///   the Reader-internal URL only when the original wasn't captured)
/// - `title`        = `title`, with a `(no title)` fallback so the article
///                    list never renders a blank row
/// - `author`       = `author`
/// - `summary`      = `summary`
/// - `image_url`    = `image_url`
/// - `content_html` = sanitized `html_content` (when present)
/// - `body_text`    = plain-text rendering of `html_content` for FTS /
///                    snippets / AI context
/// - `published_at` = `published_date ?? created_at` (RFC3339 from the API).
///                    `created_at` is the fallback so the article still
///                    has a chronological anchor for the article list when
///                    Reader couldn't extract an original publish date.
/// - `enclosures`   = empty (Reader docs are HTML, not podcast media)
pub fn document_to_article(d: &ReaderDocument) -> NewArticle {
    let title = d
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "(no title)".to_string());

    let url = d
        .source_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            d.url
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        });

    let content_html = d
        .html_content
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|h| sanitize::sanitize(h, url.as_deref()));
    let body_text = d
        .html_content
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(sanitize::html_to_text)
        .unwrap_or_default();

    let published_at = d
        .published_date
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            d.created_at
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        });

    NewArticle {
        guid: d.id.clone(),
        url,
        title,
        author: d.author.clone(),
        summary: d.summary.clone(),
        content_html,
        body_text,
        image_url: d.image_url.clone(),
        published_at,
        enclosures: Vec::new(),
    }
}

/// Extract the Reader-specific side-table fields the generic `articles`
/// schema does not carry: location, category, reading progress, the two
/// Readwise URLs, and the document's own `updated_at`. The `articles`-shaped
/// half lives in [`document_to_article`]; pair them at the upsert call site.
pub fn document_to_extra(d: &ReaderDocument) -> NewReaderDocument {
    NewReaderDocument {
        document_id: d.id.clone(),
        readwise_url: d.url.clone(),
        source_url: d.source_url.clone(),
        location: d.location.clone(),
        category: d.category.clone(),
        reading_progress: d.reading_progress,
        updated_at: d.updated_at.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn doc_json(id: &str, parent: Option<&str>) -> serde_json::Value {
        let mut obj = serde_json::json!({ "id": id });
        if let Some(p) = parent {
            obj["parent_id"] = serde_json::json!(p);
        }
        obj
    }

    // ─── mapping ────────────────────────────────────────────────

    #[test]
    fn mapping_pulls_canonical_url_from_source_then_falls_back() {
        let mut doc: ReaderDocument =
            serde_json::from_value(doc_json("doc-1", None)).unwrap();
        doc.url = Some("https://read.readwise.io/read/doc-1".into());
        doc.source_url = Some("https://example.com/post".into());
        let a = document_to_article(&doc);
        // source_url wins when present.
        assert_eq!(a.url.as_deref(), Some("https://example.com/post"));

        doc.source_url = None;
        let a = document_to_article(&doc);
        // Falls back to the Reader-internal URL when no canonical was captured.
        assert_eq!(
            a.url.as_deref(),
            Some("https://read.readwise.io/read/doc-1")
        );

        doc.url = None;
        let a = document_to_article(&doc);
        assert_eq!(a.url, None);
    }

    #[test]
    fn mapping_uses_document_id_as_guid_and_carries_metadata() {
        let mut doc: ReaderDocument =
            serde_json::from_value(doc_json("doc-42", None)).unwrap();
        doc.title = Some("A Reader Doc".into());
        doc.author = Some("Jane".into());
        doc.summary = Some("a summary".into());
        doc.image_url = Some("https://cdn.example.com/img.png".into());
        doc.published_date = Some("2025-04-01T10:00:00Z".into());

        let a = document_to_article(&doc);
        assert_eq!(a.guid, "doc-42");
        assert_eq!(a.title, "A Reader Doc");
        assert_eq!(a.author.as_deref(), Some("Jane"));
        assert_eq!(a.summary.as_deref(), Some("a summary"));
        assert_eq!(
            a.image_url.as_deref(),
            Some("https://cdn.example.com/img.png")
        );
        assert_eq!(a.published_at.as_deref(), Some("2025-04-01T10:00:00Z"));
        assert!(a.enclosures.is_empty());
    }

    #[test]
    fn mapping_falls_back_to_created_at_when_published_date_missing() {
        // Reader can't always extract an original publish date — without a
        // fallback the article list would lose its sort anchor for those
        // docs. `created_at` is the next-best chronological marker.
        let mut doc: ReaderDocument =
            serde_json::from_value(doc_json("doc-x", None)).unwrap();
        doc.title = Some("t".into());
        doc.published_date = None;
        doc.created_at = Some("2024-09-01T00:00:00Z".into());
        let a = document_to_article(&doc);
        assert_eq!(a.published_at.as_deref(), Some("2024-09-01T00:00:00Z"));
    }

    #[test]
    fn mapping_supplies_no_title_fallback_for_blank_titles() {
        // A blank title would leave the article-list row visually empty.
        let mut doc: ReaderDocument =
            serde_json::from_value(doc_json("doc-y", None)).unwrap();
        doc.title = Some("   ".into());
        let a = document_to_article(&doc);
        assert_eq!(a.title, "(no title)");

        doc.title = None;
        let a = document_to_article(&doc);
        assert_eq!(a.title, "(no title)");
    }

    #[test]
    fn mapping_sanitizes_html_content_and_derives_body_text() {
        // Reader documents arrive as raw HTML — they MUST be sanitized
        // before being persisted or rendered, exactly like RSS bodies.
        let mut doc: ReaderDocument =
            serde_json::from_value(doc_json("doc-h", None)).unwrap();
        doc.title = Some("post".into());
        doc.html_content = Some(
            "<p>Real <b>body</b>.</p><script>alert(1)</script>".into(),
        );
        let a = document_to_article(&doc);
        let html = a.content_html.expect("html_content sanitized");
        assert!(!html.contains("<script>"), "script must be stripped");
        assert!(html.contains("body"));
        assert!(a.body_text.contains("Real"));
        assert!(a.body_text.contains("body"));
    }

    #[test]
    fn mapping_leaves_html_empty_when_with_html_content_was_off() {
        let mut doc: ReaderDocument =
            serde_json::from_value(doc_json("doc-noh", None)).unwrap();
        doc.title = Some("t".into());
        doc.html_content = None;
        let a = document_to_article(&doc);
        assert!(a.content_html.is_none());
        assert!(a.body_text.is_empty());
    }

    // ─── parent-id filter ───────────────────────────────────────

    #[test]
    fn child_documents_are_recognised_as_subdocuments() {
        let parent: ReaderDocument =
            serde_json::from_value(doc_json("p1", None)).unwrap();
        let child: ReaderDocument =
            serde_json::from_value(doc_json("c1", Some("p1"))).unwrap();
        let empty_parent_id: ReaderDocument =
            serde_json::from_value(doc_json("e1", Some(""))).unwrap();
        assert!(!parent.is_subdocument());
        assert!(child.is_subdocument());
        // An empty-string `parent_id` is not a real parent reference and
        // must not strip the row from ingestion.
        assert!(!empty_parent_id.is_subdocument());
    }

    // ─── query / Retry-After ────────────────────────────────────

    #[test]
    fn build_query_emits_only_set_knobs_and_appends_cursor() {
        let opts = FetchOptions {
            updated_after: Some("2025-01-01T00:00:00Z".into()),
            location: Some("later".into()),
            category: None,
            with_html_content: true,
        };
        let q = build_query(&opts, Some("CURSOR-XYZ"));
        assert_eq!(
            q,
            vec![
                ("updatedAfter", "2025-01-01T00:00:00Z".into()),
                ("location", "later".into()),
                ("withHtmlContent", "true".into()),
                ("pageCursor", "CURSOR-XYZ".into()),
            ]
        );

        // No cursor + everything off → empty query (server defaults apply).
        let empty = build_query(&FetchOptions::default(), None);
        assert!(empty.is_empty());
    }

    #[test]
    fn parse_retry_after_understands_seconds_and_falls_back_otherwise() {
        assert_eq!(parse_retry_after(Some("7")), Duration::from_secs(7));
        assert_eq!(parse_retry_after(Some("  30  ")), Duration::from_secs(30));
        // HTTP-date form isn't emitted by Readwise; fall back rather than
        // panic so a quirky proxy can't kill the sync.
        assert_eq!(
            parse_retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
            DEFAULT_MIN_INTERVAL
        );
        assert_eq!(parse_retry_after(None), DEFAULT_MIN_INTERVAL);
    }

    // ─── pagination + 429 retry (mock transport) ────────────────

    /// Vector-backed [`PageTransport`] used to script page sequences. Each
    /// `fetch` pops one outcome from the front of the queue and records the
    /// cursor it was called with so tests can assert the loop walked the
    /// cursors correctly.
    struct MockTransport {
        outcomes: VecDeque<PageOutcome>,
        seen_cursors: Vec<Option<String>>,
    }

    impl MockTransport {
        fn new(outcomes: Vec<PageOutcome>) -> Self {
            Self {
                outcomes: outcomes.into(),
                seen_cursors: Vec::new(),
            }
        }
    }

    impl PageTransport for MockTransport {
        async fn fetch(&mut self, cursor: Option<&str>) -> AppResult<PageOutcome> {
            self.seen_cursors.push(cursor.map(str::to_string));
            self.outcomes
                .pop_front()
                .ok_or_else(|| AppError::other("mock transport exhausted"))
        }
    }

    fn doc(id: &str) -> ReaderDocument {
        serde_json::from_value(doc_json(id, None)).unwrap()
    }

    #[tokio::test]
    async fn pagination_chains_cursors_until_none() {
        let mut t = MockTransport::new(vec![
            PageOutcome::Page {
                results: vec![doc("a"), doc("b")],
                next_cursor: Some("CUR2".into()),
            },
            PageOutcome::Page {
                results: vec![doc("c")],
                next_cursor: Some("CUR3".into()),
            },
            PageOutcome::Page {
                results: vec![doc("d")],
                next_cursor: None,
            },
        ]);
        let out = paginate(&mut t).await.expect("paginate ok");
        assert_eq!(
            out.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            ["a", "b", "c", "d"]
        );
        // The loop opens with no cursor, then forwards each server-supplied
        // cursor verbatim — proves a renamed `nextPageCursor` would have
        // surfaced as a one-page truncation.
        assert_eq!(
            t.seen_cursors,
            vec![None, Some("CUR2".into()), Some("CUR3".into())]
        );
    }

    #[tokio::test]
    async fn rate_limited_response_sleeps_then_retries_same_cursor() {
        // A 429 must NOT advance the cursor — the page wasn't delivered, so
        // retrying with the next cursor would silently drop a whole page.
        let mut t = MockTransport::new(vec![
            PageOutcome::RateLimited(Duration::from_millis(1)),
            PageOutcome::Page {
                results: vec![doc("only")],
                next_cursor: None,
            },
        ]);
        let out = paginate(&mut t).await.expect("paginate ok");
        assert_eq!(out.len(), 1);
        // Both calls were made with the initial (None) cursor.
        assert_eq!(t.seen_cursors, vec![None, None]);
    }

    #[tokio::test]
    async fn repeated_rate_limits_eventually_give_up() {
        // A server that never recovers must not loop forever; we cap retries
        // and surface a stable error code the UI can localise.
        let mut outs: Vec<PageOutcome> = Vec::new();
        for _ in 0..=MAX_RETRIES + 1 {
            outs.push(PageOutcome::RateLimited(Duration::from_millis(1)));
        }
        let mut t = MockTransport::new(outs);
        let err = paginate(&mut t).await.expect_err("must error out");
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["code"], "readwiseRateLimited");
    }

    #[tokio::test]
    async fn fetch_documents_drops_subdocuments() {
        // End-to-end through `paginate` + the parent filter: a child doc
        // (highlight/note) must never reach the article shape.
        let child: ReaderDocument =
            serde_json::from_value(doc_json("c", Some("p"))).unwrap();
        let parent_a = doc("p");
        let parent_b = doc("q");
        let mut t = MockTransport::new(vec![PageOutcome::Page {
            results: vec![parent_a, child, parent_b],
            next_cursor: None,
        }]);
        let docs = paginate(&mut t).await.unwrap();
        let kept: Vec<&str> = docs
            .iter()
            .filter(|d| !d.is_subdocument())
            .map(|d| d.id.as_str())
            .collect();
        assert_eq!(kept, vec!["p", "q"]);
    }

    // ─── ListResponse decoding ──────────────────────────────────

    #[test]
    fn list_response_decodes_both_cursor_spellings() {
        let camel: ListResponse = serde_json::from_str(
            r#"{ "results": [], "nextPageCursor": "abc" }"#,
        )
        .unwrap();
        assert_eq!(camel.next_page_cursor.as_deref(), Some("abc"));

        let snake: ListResponse = serde_json::from_str(
            r#"{ "results": [], "next_page_cursor": "def" }"#,
        )
        .unwrap();
        assert_eq!(snake.next_page_cursor.as_deref(), Some("def"));
    }

    #[test]
    fn list_response_tolerates_extra_fields() {
        // The Reader API emits many more fields than we decode — a
        // schema bump on the server must not break the sync.
        let r: ListResponse = serde_json::from_str(
            r#"{
                "count": 2,
                "nextPageCursor": null,
                "results": [
                    {"id": "x", "title": "t", "extra_future_field": 1}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(r.results.len(), 1);
        assert_eq!(r.results[0].id, "x");
        assert!(r.next_page_cursor.is_none());
    }
}
