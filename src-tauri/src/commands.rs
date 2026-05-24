//! Tauri command surface — the typed IPC boundary the React frontend calls.
//! All SQL is delegated to `db`; commands only orchestrate.

use crate::ai::{self, AiConfig, AiEvent};
use crate::db::{self};
use crate::error::{AppError, AppResult};
use crate::extraction;
use crate::ingestion::discovery::{self, DiscoveryResult};
use crate::ingestion::newsletter::{self, NewsletterConfig};
use crate::ingestion::sources::{self, Normalized};
use crate::ingestion::{fetch, parse, scheduler};
use crate::models::*;
use crate::opml;
use crate::sanitize;
use crate::state::AppState;
use crate::translate;
use serde::{Deserialize, Serialize};
use tauri::{ipc::Channel, AppHandle, Emitter, Manager, State};
use url::Url;

// ─────────────────────────── folders ───────────────────────────

#[tauri::command]
pub async fn list_folders(state: State<'_, AppState>) -> AppResult<Vec<Folder>> {
    let conn = state.read().await;
    db::list_folders(&conn)
}

#[tauri::command]
pub async fn create_folder(state: State<'_, AppState>, name: String) -> AppResult<i64> {
    let conn = state.db.lock().await;
    db::create_folder(&conn, &name)
}

#[tauri::command]
pub async fn rename_folder(state: State<'_, AppState>, id: i64, name: String) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::rename_folder(&conn, id, &name)
}

#[tauri::command]
pub async fn delete_folder(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::delete_folder(&conn, id)
}

// ─────────────────────────── feeds ───────────────────────────

#[tauri::command]
pub async fn list_feeds(state: State<'_, AppState>) -> AppResult<Vec<Feed>> {
    let conn = state.read().await;
    db::list_feeds(&conn)
}

/// Subscribe to a feed. Accepts either a feed URL or a website URL (in which
/// case we auto-discover the feed). Also recognizes multi-source URLs —
/// YouTube channels, subreddits, Mastodon profiles — and rewrites them to the
/// real feed URL first (feature F5). Fetches once so the feed is immediately
/// populated, then returns the stored feed.
#[tauri::command]
pub async fn add_feed(
    state: State<'_, AppState>,
    url: String,
    folder_id: Option<i64>,
) -> AppResult<Feed> {
    let client = state.http();

    // Step 0: multi-source normalization. If the pasted URL is a known
    // special source, rewrite it to its real feed URL. A YouTube vanity URL
    // needs the channel page fetched to learn its channel id; that single
    // network call lives here (the extraction logic itself is pure).
    let (effective_url, forced_type): (String, Option<SourceType>) =
        match sources::normalize_source(&url) {
            Normalized::Feed { url, source_type } => (url, Some(source_type)),
            Normalized::NeedsYoutubeResolution { page_url } => {
                let (page_bytes, ct, _) = fetch::get(&client, &page_url).await?;
                let html = fetch::decode_html(&page_bytes, ct.as_deref());
                let channel_id = sources::extract_channel_id(&html)
                    .ok_or_else(|| AppError::code("youtubeChannelNotFound"))?;
                (
                    sources::youtube_feed_url(&channel_id),
                    Some(SourceType::Youtube),
                )
            }
            Normalized::Untouched => (url.clone(), None),
        };

    // Step 1: fetch whatever the user gave us (or the normalized feed URL).
    let (bytes, ct, final_url) = fetch::get(&client, &effective_url).await?;

    // Step 2: if it is a feed use it directly, otherwise discover one.
    let (feed_url, feed_bytes) = if parse::looks_like_feed(&bytes) {
        (final_url, bytes)
    } else {
        let html = fetch::decode_html(&bytes, ct.as_deref());
        let candidates = parse::discover_feeds(&html, &final_url);
        let candidate = candidates
            .into_iter()
            .next()
            .ok_or_else(|| AppError::code("noFeedFound"))?;
        let (fb, _, _) = fetch::get(&client, &candidate).await?;
        (candidate, fb)
    };

    // Step 3: parse and classify. A normalization step that already pinned a
    // source type (YouTube / Reddit / Mastodon) wins over heuristic detection.
    let parsed = parse::parse_feed(&feed_bytes, &feed_url)?;
    let source_type = match forced_type {
        Some(t) => t,
        None => parse::refine_source_type(
            parse::detect_source_type(&feed_url),
            &parsed,
            &feed_url,
        ),
    };

    let title = parsed
        .title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| feed_url.clone());
    let favicon = parsed.icon.clone().or_else(|| {
        parsed
            .site_url
            .as_deref()
            .and_then(|s| Url::parse(s).ok())
            .and_then(|u| u.host_str().map(String::from))
            .map(|h| format!("https://www.google.com/s2/favicons?domain={h}&sz=64"))
    });

    // Step 4: persist.
    let conn = state.db.lock().await;
    if db::find_feed_by_url(&conn, &feed_url)?.is_some() {
        return Err(AppError::code("alreadySubscribed"));
    }
    let feed_id = db::insert_feed(
        &conn,
        &feed_url,
        parsed.site_url.as_deref(),
        &title,
        parsed.description.as_deref(),
        source_type,
        folder_id,
    )?;
    if let Some(fav) = &favicon {
        db::update_feed_meta(&conn, feed_id, None, None, None, Some(fav))?;
    }
    let dedup = db::setting_flag(&conn, "dedup_enabled", false);
    let rules = db::active_rules(&conn).unwrap_or_default();
    for article in &parsed.articles {
        db::upsert_article(&conn, feed_id, article, dedup, &rules)?;
    }
    // Record that the feed was just fetched. `add_feed` fetches the document
    // here in step 1/2, so without this `last_fetched_at` would stay NULL —
    // the feed would wrongly read as "never refreshed" until the next
    // scheduler tick, and the tick would also re-fetch it in full a moment
    // after this add. (The conditional-GET revalidators are not captured —
    // `fetch::get` does not surface ETag / Last-Modified — so the next poll
    // does one full GET before it can store them; that is a single missed
    // optimisation, not incorrect behaviour, and many feeds send no ETag at
    // all.)
    let _ = db::touch_feed(&conn, feed_id);
    let last_fetched_at = db::feed_last_fetched(&conn, feed_id).ok().flatten();
    // Count actual unread rows rather than tallying insertions: keeps the
    // returned `unread_count` aligned with the sidebar's `list_feeds` count
    // regardless of how filter rules pre-set article state.
    let unread = db::count_feed_unread(&conn, feed_id)?;
    drop(conn);

    Ok(Feed {
        id: feed_id,
        feed_url,
        site_url: parsed.site_url,
        title,
        description: parsed.description,
        favicon_url: favicon,
        folder_id,
        source_type: source_type.as_str().to_string(),
        last_fetched_at,
        fetch_error: None,
        unread_count: unread,
    })
}

/// Feed discovery (feature F6). Searches the bundled curated directory for
/// entries matching `query`, scoped to `lang` (the user's UI language) so the
/// recommendations are in a language they read. When `query` looks like a URL
/// or bare domain we ALSO fetch that page and run `parse::discover_feeds` over
/// it, so the same box doubles as smart URL handling. Live page-scrape results
/// are returned first (most specific to what the user typed), then the
/// directory matches.
///
/// A failed page fetch is non-fatal — the directory results are still
/// returned — so a typo or an offline site does not break discovery.
#[tauri::command]
pub async fn search_feed_directory(
    state: State<'_, AppState>,
    query: String,
    lang: String,
) -> AppResult<Vec<DiscoveryResult>> {
    let mut results: Vec<DiscoveryResult> = Vec::new();

    // Live page scrape — only when the query genuinely looks like a URL.
    if discovery::looks_like_url(&query) {
        let target = discovery::normalize_query_url(&query);
        let client = state.http();
        if let Ok((bytes, ct, final_url)) = fetch::get(&client, &target).await {
            if parse::looks_like_feed(&bytes) {
                // The pasted URL is itself a feed — surface it directly.
                let title = parse::parse_feed(&bytes, &final_url)
                    .ok()
                    .and_then(|p| p.title);
                results.push(DiscoveryResult::from_scrape(final_url, title));
            } else {
                let html = fetch::decode_html(&bytes, ct.as_deref());
                for feed_url in parse::discover_feeds(&html, &final_url) {
                    results.push(DiscoveryResult::from_scrape(feed_url, None));
                }
            }
        }
    }

    // Curated directory matches (deduplicated against the scrape results),
    // scoped to the user's UI language so the recommendations read natively.
    for hit in discovery::search_directory(&query, &lang) {
        if !results.iter().any(|r| r.feed_url == hit.feed_url) {
            results.push(hit);
        }
    }
    Ok(results)
}

#[tauri::command]
pub async fn delete_feed(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::delete_feed(&conn, id)
}

/// Delete every locally-synced article for `id`. The subscription stays;
/// subsequent refreshes repopulate from upstream. Returns the number removed.
#[tauri::command]
pub async fn clear_feed_items(app: AppHandle, id: i64) -> AppResult<usize> {
    let n = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().await;
        db::clear_feed_items(&conn, id)?
    };
    let _ = app.emit("feeds-updated", 0);
    refresh_unread_surfaces(&app).await;
    Ok(n)
}

#[tauri::command]
pub async fn move_feed(
    state: State<'_, AppState>,
    id: i64,
    folder_id: Option<i64>,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::move_feed(&conn, id, folder_id)
}

#[tauri::command]
pub async fn rename_feed(state: State<'_, AppState>, id: i64, title: String) -> AppResult<()> {
    // `db::rename_feed` trims and rejects an empty title — the one chokepoint.
    let conn = state.db.lock().await;
    db::rename_feed(&conn, id, &title)
}

/// Refresh every feed, streaming progress to the frontend over `on_progress`.
#[tauri::command]
pub async fn refresh_feeds(
    app: AppHandle,
    on_progress: Channel<RefreshProgress>,
) -> AppResult<usize> {
    scheduler::refresh_all(&app, Some(on_progress), false).await
}

// ─────────────────────────── articles ───────────────────────────

#[tauri::command]
pub async fn list_articles(
    state: State<'_, AppState>,
    query: ArticleQuery,
    unread_only: bool,
    search: Option<String>,
    oldest_first: bool,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<ArticleSummary>> {
    let conn = state.read().await;
    db::list_articles(
        &conn,
        &query,
        unread_only,
        search.as_deref(),
        oldest_first,
        limit,
        offset,
    )
}

#[tauri::command]
pub async fn get_article(state: State<'_, AppState>, id: i64) -> AppResult<ArticleDetail> {
    let conn = state.read().await;
    db::get_article(&conn, id)
}

/// Queue a read/starred change for FreshRSS, but only when a server is linked.
fn enqueue_if_connected(conn: &rusqlite::Connection, id: i64, field: &str, value: bool) {
    if db::is_freshrss_connected(conn) {
        let _ = db::enqueue_sync(conn, id, field, value);
    }
}

/// Refresh the two unread surfaces — the Dock badge and the menu-bar tray —
/// after an operation that changed the unread count.
async fn refresh_unread_surfaces(app: &AppHandle) {
    crate::notify::update_badge(app).await;
    crate::tray::refresh(app).await;
}

#[tauri::command]
pub async fn mark_read(app: AppHandle, id: i64, read: bool) -> AppResult<()> {
    {
        let state = app.state::<AppState>();
        let conn = state.db.lock().await;
        db::set_read(&conn, id, read)?;
        enqueue_if_connected(&conn, id, "read", read);
    }
    refresh_unread_surfaces(&app).await;
    Ok(())
}

#[tauri::command]
pub async fn mark_starred(app: AppHandle, id: i64, starred: bool) -> AppResult<()> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().await;
    db::set_starred(&conn, id, starred)?;
    enqueue_if_connected(&conn, id, "starred", starred);
    Ok(())
}

#[tauri::command]
pub async fn mark_read_later(state: State<'_, AppState>, id: i64, value: bool) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_read_later(&conn, id, value)
}

#[tauri::command]
pub async fn mark_all_read(app: AppHandle, query: ArticleQuery) -> AppResult<usize> {
    let n = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().await;
        db::mark_all_read(&conn, &query, db::is_freshrss_connected(&conn))?
    };
    let _ = app.emit("feeds-updated", 0);
    refresh_unread_surfaces(&app).await;
    Ok(n)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartCounts {
    unread: i64,
    starred: i64,
    read_later: i64,
}

#[tauri::command]
pub async fn smart_counts(state: State<'_, AppState>) -> AppResult<SmartCounts> {
    let conn = state.read().await;
    let (unread, starred, read_later) = db::smart_counts(&conn)?;
    Ok(SmartCounts {
        unread,
        starred,
        read_later,
    })
}

// ─────────────────────────── full-text extraction ───────────────────────────

/// Fetch the article's source page and extract its full text (Readability).
/// Stores the result so subsequent reads are instant/offline.
#[tauri::command]
pub async fn extract_fulltext(state: State<'_, AppState>, article_id: i64) -> AppResult<String> {
    let url = {
        let conn = state.read().await;
        db::get_article(&conn, article_id)?
            .url
            .ok_or_else(|| AppError::code("noArticleUrl"))?
    };

    let http = state.http();
    let (bytes, ct, final_url) = fetch::get(&http, &url).await?;
    // Decode in the page's declared charset — a non-UTF-8 page (Shift-JIS,
    // GBK, ISO-8859-1, …) would otherwise become mojibake before Readability.
    let html = fetch::decode_html(&bytes, ct.as_deref());

    // Readability is not Send — run it on the blocking pool.
    let extracted =
        tokio::task::spawn_blocking(move || extraction::extract_article(&html, &final_url))
            .await
            .map_err(|e| AppError::other(format!("extraction task: {e}")))??;

    let conn = state.db.lock().await;
    db::set_extracted_html(&conn, article_id, &extracted)?;
    Ok(extracted)
}

/// Provider for the in-reader "fetch full text" dropdown. Both providers
/// accept a target URL appended directly to a host prefix — no percent-encoding
/// of the embedded URL.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FullTextProvider {
    /// defuddle.md returns reader-mode content rendered as Markdown by the UI.
    Defuddle,
    /// r.jina.ai returns LLM-friendly Markdown.
    Jina,
}

/// Response from `fetch_article_full_text`. `body` is the raw provider payload;
/// the frontend renders supported providers as Markdown.
#[derive(Debug, Serialize)]
pub struct FullTextResponse {
    pub provider: &'static str,
    pub body: String,
    pub source_url: String,
}

/// Compose the provider request URL. Both providers want the target URL
/// appended verbatim — no percent-encoding, matching the documented form
/// (e.g. `https://defuddle.md/https://blog.example.com/p/…`).
fn full_text_provider_url(
    provider: FullTextProvider,
    target_url: &str,
) -> (&'static str, String) {
    match provider {
        FullTextProvider::Defuddle => (
            "defuddle",
            format!("https://defuddle.md/{}", target_url),
        ),
        FullTextProvider::Jina => ("jina", format!("https://r.jina.ai/{}", target_url)),
    }
}

/// Fetch the article body through an external reader-mode service. Bypasses
/// browser CORS by going through the Tauri HTTP client, and **does not write
/// the result back to the database** — the caller renders it in-place only.
#[tauri::command]
pub async fn fetch_article_full_text(
    state: State<'_, AppState>,
    article_id: i64,
    provider: FullTextProvider,
) -> AppResult<FullTextResponse> {
    let url = {
        let conn = state.read().await;
        db::get_article(&conn, article_id)?
            .url
            .ok_or_else(|| AppError::code("noArticleUrl"))?
    };

    let (provider_id, request_url) = full_text_provider_url(provider, &url);

    let http = state.http();
    let (bytes, ct, _final_url) = fetch::get(&http, &request_url).await?;
    let body = fetch::decode_html(&bytes, ct.as_deref());
    if body.trim().is_empty() {
        return Err(AppError::code("emptyFullTextResponse"));
    }
    Ok(FullTextResponse {
        provider: provider_id,
        body,
        source_url: url,
    })
}

// ─────────────────────────── OPML ───────────────────────────

#[tauri::command]
pub async fn import_opml(app: AppHandle, content: String) -> AppResult<usize> {
    let imported = opml::parse(&content)?;
    let count = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().await;
        // One transaction for the whole import — a mid-list failure rolls
        // back rather than leaving feeds (and auto-created folders) partly
        // imported.
        let tx = conn.unchecked_transaction()?;
        let mut added = 0;
        for feed in imported {
            if db::find_feed_by_url(&tx, &feed.feed_url)?.is_some() {
                continue;
            }
            let folder_id = match &feed.folder {
                Some(name) => Some(db::folder_id_by_name(&tx, name)?),
                None => None,
            };
            let source_type = parse::detect_source_type(&feed.feed_url);
            db::insert_feed(
                &tx,
                &feed.feed_url,
                None,
                &feed.title,
                None,
                source_type,
                folder_id,
            )?;
            added += 1;
        }
        tx.commit()?;
        added
    };
    // Newly imported feeds have no articles yet — kick off a refresh. Pass
    // wait_if_busy so it queues behind any in-flight refresh instead of
    // skipping and leaving the imported feeds empty until the next tick.
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = scheduler::refresh_all(&app2, None, true).await;
    });
    Ok(count)
}

#[tauri::command]
pub async fn export_opml(state: State<'_, AppState>) -> AppResult<String> {
    let conn = state.read().await;
    let feeds = db::feeds_for_export(&conn)?;
    opml::build(&feeds)
}

// ─────────────────────────── settings ───────────────────────────

#[tauri::command]
pub async fn get_setting(state: State<'_, AppState>, key: String) -> AppResult<Option<String>> {
    let conn = state.read().await;
    db::get_setting(&conn, &key)
}

#[tauri::command]
pub async fn set_setting(
    state: State<'_, AppState>,
    key: String,
    value: String,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_setting(&conn, &key, &value)
}

// ─────────────────────────── AI ───────────────────────────

/// Load the AI provider configuration from the settings table.
fn load_ai_config(conn: &rusqlite::Connection) -> AppResult<AiConfig> {
    AiConfig::new(
        db::get_setting(conn, "ai_provider")?,
        db::get_setting(conn, "ai_api_key")?,
        db::get_setting(conn, "ai_model")?,
        db::get_setting(conn, "ai_base_url")?,
    )
}

/// Truncate to at most `max` characters without splitting a UTF-8 boundary.
fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// A system-prompt directive so AI output matches the UI language rather than
/// defaulting to whatever language the source article happens to be in.
fn response_language(conn: &rusqlite::Connection) -> &'static str {
    match db::get_setting(conn, "language").ok().flatten().as_deref() {
        Some("zh") => "\n\nAlways write your response in Simplified Chinese.",
        Some("ja") => "\n\nAlways write your response in Japanese.",
        _ => "\n\nAlways write your response in English.",
    }
}

/// The article-translation target language code: the dedicated
/// `translate_target_lang` setting, falling back to the UI `language`, then
/// English. Stored as a code (`en` / `zh` / `ja`); `translate::language_name`
/// maps it to the name used in the prompt.
fn translate_target_lang(conn: &rusqlite::Connection) -> String {
    db::get_setting(conn, "translate_target_lang")
        .ok()
        .flatten()
        .or_else(|| db::get_setting(conn, "language").ok().flatten())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "en".to_string())
}

/// Stream an AI summary of one article; the full summary is also persisted.
#[tauri::command]
pub async fn ai_summarize(
    state: State<'_, AppState>,
    article_id: i64,
    on_token: Channel<AiEvent>,
) -> AppResult<()> {
    let (title, body, cfg, lang) = {
        let conn = state.read().await;
        let (title, body) = db::article_text(&conn, article_id)?;
        (title, body, load_ai_config(&conn)?, response_language(&conn))
    };
    // A title-only item (link-aggregator posts, some podcast/video feeds carry
    // no body text) gives the model nothing to summarize. Without this guard it
    // would invent a "summary" from the bare title alone — and that fabricated
    // text would then be persisted to `ai_summary`. Bail out the same way
    // `ai_ask` / `ai_digest` do when their input is empty.
    if body.trim().is_empty() {
        return Err(AppError::code("noArticleBody"));
    }
    // The drawer renders the response as markdown (.ai-prose styles paragraphs,
    // bullets, and bold), so we ask for structured output instead of a single
    // dense paragraph — the reader can scan a TL;DR + bullets far faster.
    let system = format!(
        "You are a sharp news editor. Summarize the article so a reader can \
         decide whether to read it in full.\n\n\
         Format the response in markdown using exactly this shape:\n\
         **TL;DR** — One sentence capturing the single most important point.\n\n\
         - Key fact, finding, or claim (under ~20 words)\n\
         - Another key point\n\
         - 3 to 5 bullets total, one idea each, no nested bullets\n\n\
         Output only this structure. No preamble, no closing remarks, no \
         section headers, no extra prose.{lang}"
    );
    let user = format!("Title: {title}\n\n{}", truncate(&body, 8000));

    let http = state.http();
    let outcome = ai::stream_chat(&http, &cfg, &system, &user, &on_token, ai::MAX_TOKENS).await?;
    // Persist only a summary that streamed to completion. If the user closed
    // the AI panel mid-stream the channel was dropped and `outcome.text` holds
    // just a truncated fragment — caching that would make the next open show a
    // broken half-summary with no way to regenerate it.
    if outcome.completed && !outcome.text.trim().is_empty() {
        let conn = state.db.lock().await;
        db::set_ai_summary(&conn, article_id, outcome.text.trim())?;
    }
    Ok(())
}

/// Answer a question using the user's subscribed articles as RAG context.
/// Retrieval currently uses FTS5 keyword search (semantic search is Phase 5).
#[tauri::command]
pub async fn ai_ask(
    state: State<'_, AppState>,
    question: String,
    on_token: Channel<AiEvent>,
) -> AppResult<()> {
    let (cfg, context, lang) = {
        let conn = state.read().await;
        let cfg = load_ai_config(&conn)?;
        // RAG retrieval is recall-oriented: match articles that share *any* of
        // the question's keywords. `list_articles` AND-joins every search word,
        // which for a natural-language question matches nothing.
        let hits = db::search_articles_for_rag(&conn, &question, 6)?;
        let mut context = String::new();
        for (id, _title, feed_title) in hits {
            let (title, body) = db::article_text(&conn, id)?;
            context.push_str(&format!(
                "## {} — {}\n{}\n\n",
                title,
                feed_title,
                truncate(&body, 1200)
            ));
        }
        (cfg, context, response_language(&conn))
    };

    let system = format!(
        "You answer the user's question using only the provided \
         articles from their RSS subscriptions. Cite the article \
         titles you draw from. If the articles do not contain the \
         answer, say so plainly.{lang}"
    );
    let user = if context.trim().is_empty() {
        format!("No relevant articles were found.\n\nQuestion: {question}")
    } else {
        format!("Articles from the user's feeds:\n\n{context}---\n\nQuestion: {question}")
    };

    let http = state.http();
    ai::stream_chat(&http, &cfg, &system, &user, &on_token, ai::MAX_TOKENS).await?;
    Ok(())
}

/// Stream an AI briefing that synthesizes the most recent articles by theme.
#[tauri::command]
pub async fn ai_digest(
    state: State<'_, AppState>,
    on_token: Channel<AiEvent>,
) -> AppResult<()> {
    let (cfg, articles, lang) = {
        let conn = state.read().await;
        (
            load_ai_config(&conn)?,
            db::digest_source(&conn, 30)?,
            response_language(&conn),
        )
    };
    if articles.is_empty() {
        return Err(AppError::code("noArticles"));
    }

    let mut corpus = String::new();
    for (title, feed, text) in &articles {
        corpus.push_str(&format!("- [{feed}] {title}: {}\n", truncate(text, 400)));
    }

    let system = format!(
        "You are the user's personal news briefer. From the recent \
         articles, write a crisp briefing: group related items into \
         2-4 themed sections with short headers, lead with what \
         matters most, and keep it skimmable. Plain prose, no preamble.{lang}"
    );
    let user = format!("Recent articles from my feeds:\n\n{corpus}");

    let http = state.http();
    ai::stream_chat(&http, &cfg, &system, &user, &on_token, ai::MAX_TOKENS).await?;
    Ok(())
}

/// Progress events streamed to the frontend during a translation. Reported once
/// per batch (a group of whole blocks), never per token: token-level IPC across
/// a full article would flood the webview's main thread and freeze the UI.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase", tag = "type", content = "data")]
pub enum TranslateEvent {
    /// The number of batches the body was split into, sent before any batch.
    Start { total: usize },
    /// One freshly translated, sanitized batch of HTML, plus how many batches
    /// have completed so far.
    Batch { html: String, done: usize },
    /// The full sanitized translation, sent once on completion.
    Done { html: String },
}

/// Translate one article's body into the configured target language, reporting
/// progress per batch over `on_event`. The body is split into batches of whole
/// blocks (so long articles and the per-request token cap are both handled) and
/// translated batch by batch with the HTML structure preserved. The reassembled,
/// sanitized result is cached on the article row and reused on the next open.
///
/// The work runs to completion independently of the reader view, so the frontend
/// can start several translations at once and switch articles without
/// interrupting any of them.
#[tauri::command]
pub async fn ai_translate(
    state: State<'_, AppState>,
    article_id: i64,
    on_event: Channel<TranslateEvent>,
) -> AppResult<()> {
    let (source_html, cfg, target) = {
        let conn = state.read().await;
        let detail = db::get_article(&conn, article_id)?;
        // Translate the richest body available: the extracted full text when the
        // user has run extraction, otherwise the feed's own HTML.
        let source = detail
            .extracted_html
            .filter(|s| !s.trim().is_empty())
            .or(detail.content_html)
            .unwrap_or_default();
        (source, load_ai_config(&conn)?, translate_target_lang(&conn))
    };
    if source_html.trim().is_empty() {
        return Err(AppError::code("noArticleBody"));
    }

    let batches = translate::chunk_blocks(&source_html, ai::TRANSLATE_CHUNK_BUDGET);
    let total = batches.len();
    let _ = on_event.send(TranslateEvent::Start { total });

    let system = translate::translate_system_prompt(translate::language_name(&target));
    let http = state.http();
    let mut full = String::new();
    for (i, batch) in batches.iter().enumerate() {
        let text = ai::complete_chat(&http, &cfg, &system, batch, ai::TRANSLATE_MAX_TOKENS).await?;
        // The model output is untrusted, so each batch passes through the same
        // sanitizer as feed HTML before it reaches the webview or the database.
        // Source URLs are already absolute (sanitized at ingestion), so no base
        // is needed.
        let clean = sanitize::sanitize(translate::strip_code_fence(&text).trim(), None);
        full.push_str(&clean);
        full.push('\n');
        let _ = on_event.send(TranslateEvent::Batch { html: clean, done: i + 1 });
    }

    let final_html = full.trim().to_string();
    if !final_html.is_empty() {
        let conn = state.db.lock().await;
        db::set_translation(&conn, article_id, &final_html, &target)?;
    }
    let _ = on_event.send(TranslateEvent::Done { html: final_html });
    Ok(())
}

// ─────────────────────────── storage ───────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStats {
    db_bytes: i64,
    article_count: i64,
    feed_count: i64,
}

#[tauri::command]
pub async fn storage_stats(state: State<'_, AppState>) -> AppResult<StorageStats> {
    let conn = state.read().await;
    let (db_bytes, article_count, feed_count) = db::storage_stats(&conn)?;
    Ok(StorageStats {
        db_bytes,
        article_count,
        feed_count,
    })
}

/// Delete read articles older than `days` (starred / read-later are kept).
#[tauri::command]
pub async fn cleanup_articles(app: AppHandle, days: i64) -> AppResult<usize> {
    let n = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().await;
        db::cleanup_old_articles(&conn, days)?
    };
    let _ = app.emit("feeds-updated", 0);
    Ok(n)
}

/// Reclaim free database pages.
#[tauri::command]
pub async fn vacuum_db(state: State<'_, AppState>) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::vacuum(&conn)
}

/// Clear every stored setting (AI keys, sync credentials, preferences).
#[tauri::command]
pub async fn reset_settings(state: State<'_, AppState>) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::reset_settings(&conn)
}

/// Delete all feeds, folders and articles. Irreversible.
#[tauri::command]
pub async fn clear_all_data(app: AppHandle) -> AppResult<()> {
    {
        let state = app.state::<AppState>();
        let conn = state.db.lock().await;
        db::clear_all_data(&conn)?;
    }
    let _ = app.emit("feeds-updated", 0);
    refresh_unread_surfaces(&app).await;
    Ok(())
}

// ─────────────────────────── network ───────────────────────────

/// Rebuild the HTTP client from the persisted proxy / timeout settings so the
/// change takes effect without an app restart.
#[tauri::command]
pub async fn apply_network_settings(state: State<'_, AppState>) -> AppResult<()> {
    let client = {
        // Pure settings read — use the read pool, not the writer lock.
        let conn = state.read().await;
        fetch::build_client_from_settings(&conn)
    };
    state.set_http(client);
    Ok(())
}

// ─────────────────────────── FreshRSS sync ───────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreshRssStatus {
    connected: bool,
    url: Option<String>,
}

#[tauri::command]
pub async fn freshrss_connect(
    app: AppHandle,
    url: String,
    username: String,
    password: String,
) -> AppResult<()> {
    crate::sync::connect(&app, &url, &username, &password).await
}

#[tauri::command]
pub async fn freshrss_disconnect(app: AppHandle) -> AppResult<()> {
    crate::sync::disconnect(&app).await
}

#[tauri::command]
pub async fn freshrss_status(app: AppHandle) -> AppResult<FreshRssStatus> {
    let url = crate::sync::connected_url(&app).await?;
    Ok(FreshRssStatus {
        connected: url.is_some(),
        url,
    })
}

/// Run a full FreshRSS sync now; returns the number of reconciled articles.
#[tauri::command]
pub async fn freshrss_sync(app: AppHandle) -> AppResult<usize> {
    let n = crate::sync::sync_now(&app).await?;
    let _ = app.emit("feeds-updated", 0);
    refresh_unread_surfaces(&app).await;
    Ok(n)
}

/// Rebuild the tray menu — used after a language change.
#[tauri::command]
pub async fn refresh_tray(app: AppHandle) -> AppResult<()> {
    crate::tray::refresh(&app).await;
    Ok(())
}

// ─────────────────────────── Readwise Reader ───────────────────────────

/// Whether a Readwise API token is currently stored. Reported through
/// `readwise_get_token_status` so the Settings panel can show "Set ✓" /
/// "Not set" without ever pulling the token itself across the IPC boundary.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadwiseTokenStatus {
    pub has_token: bool,
}

/// Pin a new Readwise API token. Stored under the `readwise_token` settings
/// key shared with the highlights integration; trims surrounding whitespace
/// so a copy-paste with a stray newline still saves correctly. Empty input
/// is rejected so the UI's "Clear" button is the only path to a tombstoned
/// value — saving "" silently would look like a successful save but disable
/// every Readwise sync.
#[tauri::command]
pub async fn readwise_set_token(state: State<'_, AppState>, token: String) -> AppResult<()> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(AppError::code("emptyReadwiseToken"));
    }
    let conn = state.db.lock().await;
    db::set_setting(&conn, crate::readwise_reader::TOKEN_SETTING, trimmed)
}

/// Check whether a Readwise token is currently configured. Never returns
/// the token itself — only a presence flag — so the renderer cannot accidentally
/// log or display the plaintext value.
#[tauri::command]
pub async fn readwise_get_token_status(
    state: State<'_, AppState>,
) -> AppResult<ReadwiseTokenStatus> {
    let conn = state.read().await;
    let has_token = crate::readwise_reader::read_token(&conn)?.is_some();
    Ok(ReadwiseTokenStatus { has_token })
}

/// Tombstone the stored token by writing an empty string. `read_token`
/// already treats `""` as "no token", so existing sync paths fall back to
/// the `noReadwiseToken` error without further changes.
#[tauri::command]
pub async fn readwise_clear_token(state: State<'_, AppState>) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_setting(&conn, crate::readwise_reader::TOKEN_SETTING, "")
}

/// Verify the stored token by issuing a single 1-row Reader list request.
/// Maps 401/403 to a localisable `readwiseTokenInvalid` code so the UI can
/// render a translated message instead of dumping the raw HTTP error. Any
/// other transport failure surfaces as the normal `network` error.
#[tauri::command]
pub async fn readwise_test_token(state: State<'_, AppState>) -> AppResult<()> {
    use crate::readwise_reader;

    let token = {
        let conn = state.read().await;
        readwise_reader::read_token(&conn)?
            .ok_or_else(|| AppError::code("noReadwiseToken"))?
    };

    // A single-page, no-html, `later` probe is the cheapest call that still
    // exercises auth. We discard the result; we only care that the request
    // didn't 401/403.
    let opts = readwise_reader::FetchOptions {
        location: Some("later".to_string()),
        ..Default::default()
    };
    let http = state.http();
    match readwise_reader::fetch_documents(&http, &token, &opts).await {
        Ok(_) => Ok(()),
        Err(AppError::Http(e)) => {
            if matches!(
                e.status(),
                Some(reqwest::StatusCode::UNAUTHORIZED) | Some(reqwest::StatusCode::FORBIDDEN)
            ) {
                Err(AppError::code("readwiseTokenInvalid"))
            } else {
                Err(AppError::Http(e))
            }
        }
        Err(other) => Err(other),
    }
}

/// Pull the user's Readwise Reader document list and upsert each parent doc
/// into the synthetic Readwise feed. Returns the number of *new* documents
/// added on this run (existing docs are still updated in place — see
/// `db::upsert_readwise_document` — but only the genuinely-new tally is what
/// the "N new" toast wants to display, matching how the RSS scheduler counts).
///
/// `category` filters which Reader category to pull (`article` / `email` /
/// `rss` / `highlight` / `note` / `pdf` / `epub` / `tweet` / `video`);
/// `None` means no category filter (pull everything). `with_html` toggles
/// the costly `withHtmlContent=true` request flag. Both are decided by the
/// UI; the command itself stays oblivious so future settings plumb through
/// without touching the IPC surface.
///
/// The HTTP fetch and the DB write are intentionally split so the writer lock
/// is never held across the (slow, 20-req/min throttled) network pull —
/// otherwise a multi-page sync would block every UI read for the duration of
/// the pull, which under WAL is the whole point of separating writer / reader
/// connections.
#[tauri::command]
pub async fn readwise_reader_sync(
    app: AppHandle,
    category: Option<String>,
    with_html: bool,
) -> AppResult<usize> {
    use crate::readwise_reader;

    let state = app.state::<AppState>();

    // 1. Read the token (settings-table). Empty / missing tokens short-
    //    circuit with the same code the highlights integration uses, so the
    //    i18n layer renders a single "no token" message.
    let token = {
        let conn = state.read().await;
        readwise_reader::read_token(&conn)?
            .ok_or_else(|| AppError::code("noReadwiseToken"))?
    };

    // 2. Pull every parent document matching the category filter. The client
    //    handles cursor pagination, 20-req/min throttling and 429 backoff.
    let opts = readwise_reader::FetchOptions {
        category,
        with_html_content: with_html,
        ..Default::default()
    };
    let http = state.http();
    let docs = readwise_reader::fetch_documents(&http, &token, &opts).await?;

    // 3. Upsert each document under the synthetic Readwise feed. Run all of
    //    them under one writer lock — they're small writes against a single
    //    feed and a transaction-per-doc avoids holding the writer for the
    //    whole HTTP pull above.
    let new_count = {
        let conn = state.db.lock().await;
        let feed_id = db::readwise_feed_id(&conn)?;
        let mut new = 0usize;
        for d in &docs {
            let a = readwise_reader::document_to_article(d);
            let x = readwise_reader::document_to_extra(d);
            if db::upsert_readwise_document(&conn, feed_id, &a, &x)? {
                new += 1;
            }
        }
        new
    };

    // 4. The DB has changed — tell the UI to re-render and re-poll its smart
    //    counts. `feeds-updated` is the same event the RSS scheduler emits;
    //    the frontend already listens for it, so Reader docs show up
    //    immediately without a sidebar-specific signal. The unread-surfaces
    //    refresh (Dock badge / tray) covers a sync that lands brand-new
    //    unread documents.
    let _ = app.emit("feeds-updated", new_count as u64);
    refresh_unread_surfaces(&app).await;

    Ok(new_count)
}

/// Drain a `papr://subscribe` URL that was delivered before the webview could
/// receive the `deep-link-subscribe` event (a cold-start launch). The frontend
/// calls this once on mount; returns `None` when there is nothing pending.
#[tauri::command]
pub async fn take_pending_deep_link(state: State<'_, AppState>) -> AppResult<Option<String>> {
    Ok(state.take_pending_deep_link())
}

// ─────────────────────────── tags ───────────────────────────

#[tauri::command]
pub async fn list_tags(state: State<'_, AppState>) -> AppResult<Vec<Tag>> {
    let conn = state.read().await;
    db::list_tags(&conn)
}

#[tauri::command]
pub async fn create_tag(state: State<'_, AppState>, name: String) -> AppResult<i64> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::code("emptyTagName"));
    }
    let conn = state.db.lock().await;
    db::create_tag(&conn, name)
}

#[tauri::command]
pub async fn rename_tag(state: State<'_, AppState>, id: i64, name: String) -> AppResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::code("emptyTagName"));
    }
    let conn = state.db.lock().await;
    db::rename_tag(&conn, id, name)
}

#[tauri::command]
pub async fn set_tag_color(
    state: State<'_, AppState>,
    id: i64,
    color: String,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_tag_color(&conn, id, &color)
}

#[tauri::command]
pub async fn delete_tag(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::delete_tag(&conn, id)
}

/// Attach or detach a tag from one article.
#[tauri::command]
pub async fn set_article_tag(
    state: State<'_, AppState>,
    article_id: i64,
    tag_id: i64,
    on: bool,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_article_tag(&conn, article_id, tag_id, on)
}

// ─────────────────────────── filter rules ───────────────────────────

#[tauri::command]
pub async fn list_rules(state: State<'_, AppState>) -> AppResult<Vec<Rule>> {
    let conn = state.read().await;
    db::list_rules(&conn)
}

#[tauri::command]
pub async fn create_rule(
    state: State<'_, AppState>,
    name: String,
    feed_id: Option<i64>,
    field: String,
    query: String,
    action: String,
) -> AppResult<i64> {
    if query.trim().is_empty() {
        return Err(AppError::code("emptyRuleQuery"));
    }
    let conn = state.db.lock().await;
    db::create_rule(&conn, name.trim(), feed_id, &field, query.trim(), &action)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_rule(
    state: State<'_, AppState>,
    id: i64,
    name: String,
    enabled: bool,
    feed_id: Option<i64>,
    field: String,
    query: String,
    action: String,
) -> AppResult<()> {
    if query.trim().is_empty() {
        return Err(AppError::code("emptyRuleQuery"));
    }
    let conn = state.db.lock().await;
    db::update_rule(&conn, id, name.trim(), enabled, feed_id, &field, query.trim(), &action)
}

#[tauri::command]
pub async fn delete_rule(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::delete_rule(&conn, id)
}

/// Apply a rule's action to the already-stored articles it matches and return
/// how many were acted on. The frontend calls this right after saving a rule so
/// an enabled rule affects the existing backlog, not just future articles. A
/// `skip` rule deletes its matches, so the UI confirms before invoking this.
#[tauri::command]
pub async fn apply_rule_to_existing(
    state: State<'_, AppState>,
    feed_id: Option<i64>,
    field: String,
    query: String,
    action: String,
) -> AppResult<usize> {
    if query.trim().is_empty() {
        return Err(AppError::code("emptyRuleQuery"));
    }
    let conn = state.db.lock().await;
    db::apply_rule_to_existing(&conn, feed_id, &field, query.trim(), &action)
}

/// Persist a reordered tag list (ids in the new display order).
#[tauri::command]
pub async fn reorder_tags(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::reorder_tags(&conn, &ids)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RulePreview {
    count: i64,
    samples: Vec<String>,
}

/// Dry-run a draft rule against already-stored articles.
#[tauri::command]
pub async fn preview_rule(
    state: State<'_, AppState>,
    feed_id: Option<i64>,
    field: String,
    query: String,
) -> AppResult<RulePreview> {
    let conn = state.read().await;
    let (count, samples) = db::preview_rule(&conn, feed_id, &field, query.trim())?;
    Ok(RulePreview { count, samples })
}

// ─────────────────────────── newsletter sources ───────────────────────────

/// A configured email-newsletter source, as shown in the UI (no password).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsletterSource {
    feed_id: i64,
    title: String,
    host: String,
    port: u16,
    username: String,
    folder: String,
}

/// Payload for `add_newsletter_source` — the IMAP mailbox to start polling.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsletterInput {
    /// A display name for the source (falls back to the username).
    pub title: Option<String>,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// IMAP app-password / token. Stored in the local DB only.
    pub password: String,
    /// Mailbox to poll, e.g. `INBOX` or `Newsletters`.
    pub folder: String,
}

/// Add an email-newsletter source. Verifies the IMAP credentials by polling
/// the mailbox once, ingests whatever it finds, and persists the source so the
/// background scheduler keeps polling it. Backed by a `feeds` row plus an
/// entry in `newsletter_sources` (see migration #10).
#[tauri::command]
pub async fn add_newsletter_source(
    state: State<'_, AppState>,
    input: NewsletterInput,
) -> AppResult<Feed> {
    let cfg = NewsletterConfig {
        host: input.host.trim().to_string(),
        port: input.port,
        username: input.username.trim().to_string(),
        password: input.password.clone(),
        folder: {
            let f = input.folder.trim();
            if f.is_empty() { "INBOX".to_string() } else { f.to_string() }
        },
    };
    if cfg.host.is_empty() || cfg.username.is_empty() || cfg.password.is_empty() {
        return Err(AppError::code("newsletterMissingFields"));
    }
    let feed_url = newsletter::synthetic_feed_url(&cfg);
    let title = input
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .unwrap_or_else(|| cfg.username.clone());

    // Reject a duplicate mailbox before doing the (slow) IMAP round-trip.
    {
        let conn = state.read().await;
        if db::find_feed_by_url(&conn, &feed_url)?.is_some() {
            return Err(AppError::code("alreadySubscribed"));
        }
    }

    // Verify the credentials by actually connecting. The `imap` crate is
    // blocking, so the connection runs on the blocking pool — and it has no
    // per-operation timeout, so a server that completes the TCP/TLS handshake
    // but then stalls mid-command would block this command forever (the Add
    // dialog spinner never resolves, the blocking worker thread is leaked).
    // Bound the whole probe with the same wall-clock cap the scheduler's
    // background poll uses, so a wedged mailbox degrades to a clean error.
    let probe_cfg = cfg.clone();
    let messages = match tokio::time::timeout(
        std::time::Duration::from_secs(scheduler::NEWSLETTER_POLL_TIMEOUT_SECS),
        tokio::task::spawn_blocking(move || newsletter::fetch_recent(&probe_cfg, 30)),
    )
    .await
    {
        Ok(joined) => joined
            .map_err(|e| AppError::other(format!("newsletter poll task: {e}")))??,
        Err(_) => return Err(AppError::code("newsletterPollTimeout")),
    };

    // Persist the source, then ingest the messages just fetched.
    let conn = state.db.lock().await;
    let feed_id = db::insert_newsletter_source(&conn, &feed_url, &title, &cfg)?;
    let rules = db::active_rules(&conn).unwrap_or_default();
    // `upsert_article` returns `true` only for genuinely new *unread* rows, so
    // articles a `read` rule pre-marked read are correctly excluded from the
    // returned `unread_count` (matching the sidebar's `list_feeds` count).
    let mut unread = 0i64;
    for raw in &messages {
        if let Some(parsed) = newsletter::email_to_article(raw) {
            if db::upsert_article(&conn, feed_id, &parsed.article, false, &rules)? {
                unread += 1;
            }
        }
    }
    // Record that the mailbox was just polled. The IMAP fetch above is a
    // genuine, successful refresh of this source — without this the feed's
    // `last_fetched_at` stays NULL and the sidebar reads it as "never
    // refreshed" until the next scheduler tick (up to the refresh interval
    // away). Mirrors `touch_feed` in `scheduler::poll_newsletters` for the
    // background poll, and the same handling `add_feed` applies.
    let _ = db::touch_feed(&conn, feed_id);
    let last_fetched_at = db::feed_last_fetched(&conn, feed_id).ok().flatten();
    drop(conn);

    Ok(Feed {
        id: feed_id,
        feed_url,
        site_url: None,
        title,
        description: None,
        favicon_url: None,
        folder_id: None,
        source_type: SourceType::Newsletter.as_str().to_string(),
        last_fetched_at,
        fetch_error: None,
        unread_count: unread,
    })
}

/// Every configured newsletter source (passwords omitted).
#[tauri::command]
pub async fn list_newsletter_sources(
    state: State<'_, AppState>,
) -> AppResult<Vec<NewsletterSource>> {
    let conn = state.read().await;
    Ok(db::list_newsletter_sources(&conn)?
        .into_iter()
        .map(|r| NewsletterSource {
            feed_id: r.feed_id,
            title: r.title,
            host: r.host,
            port: r.port,
            username: r.username,
            folder: r.folder,
        })
        .collect())
}

/// Remove a newsletter source and all of its ingested articles.
#[tauri::command]
pub async fn remove_newsletter_source(
    state: State<'_, AppState>,
    feed_id: i64,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::delete_newsletter_source(&conn, feed_id)
}

// ─────────────────────────── highlights (F7) ───────────────────────────

/// Create a highlight on an article. The frontend supplies the quote plus its
/// anchoring context (prefix / suffix window and the plain-text offset).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn create_highlight(
    state: State<'_, AppState>,
    article_id: i64,
    quote: String,
    prefix: String,
    suffix: String,
    text_offset: i64,
    color: String,
    note: String,
) -> AppResult<i64> {
    if quote.trim().is_empty() {
        return Err(AppError::code("emptyHighlight"));
    }
    let conn = state.db.lock().await;
    db::insert_highlight(
        &conn,
        &db::NewHighlight {
            article_id,
            quote: &quote,
            prefix: &prefix,
            suffix: &suffix,
            text_offset,
            color: &color,
            note: &note,
        },
    )
}

/// Every highlight on one article, in reading order.
#[tauri::command]
pub async fn list_highlights(
    state: State<'_, AppState>,
    article_id: i64,
) -> AppResult<Vec<Highlight>> {
    let conn = state.read().await;
    db::list_highlights(&conn, article_id)
}

/// Every highlight across all articles — for the Highlights browser.
#[tauri::command]
pub async fn list_all_highlights(state: State<'_, AppState>) -> AppResult<Vec<Highlight>> {
    let conn = state.read().await;
    db::list_all_highlights(&conn)
}

/// Replace a highlight's note (an empty string clears it).
#[tauri::command]
pub async fn update_highlight_note(
    state: State<'_, AppState>,
    id: i64,
    note: String,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::update_highlight_note(&conn, id, &note)
}

/// Change a highlight's colour (a palette key).
#[tauri::command]
pub async fn set_highlight_color(
    state: State<'_, AppState>,
    id: i64,
    color: String,
) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::set_highlight_color(&conn, id, &color)
}

#[tauri::command]
pub async fn delete_highlight(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    let conn = state.db.lock().await;
    db::delete_highlight(&conn, id)
}

#[cfg(test)]
mod tests {
    use super::{full_text_provider_url, FullTextProvider};

    #[test]
    fn defuddle_appends_target_url_verbatim() {
        let (id, url) = full_text_provider_url(
            FullTextProvider::Defuddle,
            "https://blog.bytebytego.com/p/some-article",
        );
        assert_eq!(id, "defuddle");
        // Critical: no percent-encoding of the embedded URL.
        assert_eq!(
            url,
            "https://defuddle.md/https://blog.bytebytego.com/p/some-article"
        );
    }

    #[test]
    fn jina_appends_target_url_verbatim() {
        let (id, url) = full_text_provider_url(
            FullTextProvider::Jina,
            "https://example.com/posts/42?a=1&b=2",
        );
        assert_eq!(id, "jina");
        assert_eq!(
            url,
            "https://r.jina.ai/https://example.com/posts/42?a=1&b=2"
        );
    }
}
