//! HTTP fetching with conditional GET (ETag / If-Modified-Since) so unchanged
//! feeds cost a single 304 round-trip.

use crate::db;
use crate::error::{AppError, AppResult};
use reqwest::header::{
    CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
};
use reqwest::{Client, StatusCode};
use rusqlite::Connection;
use std::time::Duration;

pub const USER_AGENT: &str = "Papr/0.1 (+https://github.com/papr-reader)";

/// Hard cap on a fetched body. Feeds and article pages are text — a few
/// hundred KB at most — so 16 MiB is generous while still stopping a
/// hostile or misconfigured server from exhausting memory.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Read a response body, aborting if it exceeds `MAX_BODY_BYTES`. Streams in
/// chunks so an unbounded (or lying-`Content-Length`) response can't first be
/// buffered whole.
async fn read_capped(mut resp: reqwest::Response) -> AppResult<Vec<u8>> {
    if resp.content_length().is_some_and(|n| n > MAX_BODY_BYTES as u64) {
        return Err(AppError::code("responseTooLarge"));
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if buf.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(AppError::code("responseTooLarge"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Build the shared HTTP client (connection pooling, gzip/brotli, redirects).
///
/// `timeout_secs` bounds the whole request. `proxy` is one of `"system"`
/// (honour `HTTP(S)_PROXY` env vars), `"none"` (bypass all proxies), or an
/// explicit proxy URL.
pub fn build_client(timeout_secs: u64, proxy: &str) -> Client {
    let mut builder = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(timeout_secs.clamp(5, 300)))
        .connect_timeout(Duration::from_secs(10));

    match proxy {
        "system" | "" => {}
        "none" => builder = builder.no_proxy(),
        custom => {
            if let Ok(p) = reqwest::Proxy::all(custom) {
                builder = builder.proxy(p);
            }
        }
    }
    builder.build().expect("failed to build reqwest client")
}

/// Build the HTTP client from the persisted network settings.
pub fn build_client_from_settings(conn: &Connection) -> Client {
    let timeout = db::get_setting(conn, "net_timeout_sec")
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let proxy = db::get_setting(conn, "net_proxy")
        .ok()
        .flatten()
        .unwrap_or_else(|| "system".to_string());
    build_client(timeout, &proxy)
}

/// Result of a conditional GET against a feed URL.
pub enum Fetched {
    /// Server returned 304 — the stored copy is still current.
    NotModified,
    /// Fresh content, along with revalidation headers to store.
    Body {
        bytes: Vec<u8>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

/// Conditional GET. Sends `If-None-Match`/`If-Modified-Since` when we have them.
pub async fn conditional_get(
    client: &Client,
    url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> AppResult<Fetched> {
    let mut req = client.get(url);
    if let Some(e) = etag {
        req = req.header(IF_NONE_MATCH, e);
    }
    if let Some(lm) = last_modified {
        req = req.header(IF_MODIFIED_SINCE, lm);
    }

    let resp = req.send().await?;
    if resp.status() == StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    let resp = resp.error_for_status()?;
    let header = |name: reqwest::header::HeaderName| {
        resp.headers()
            .get(&name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    };
    let etag = header(ETAG);
    let last_modified = header(LAST_MODIFIED);
    let bytes = read_capped(resp).await?;
    Ok(Fetched::Body {
        bytes,
        etag,
        last_modified,
    })
}

/// Plain GET returning `(body, content_type, final_url)` — used for feed
/// auto-discovery and full-text article extraction.
pub async fn get(client: &Client, url: &str) -> AppResult<(Vec<u8>, Option<String>, String)> {
    let resp = client.get(url).send().await?.error_for_status()?;
    let final_url = resp.url().to_string();
    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let bytes = read_capped(resp).await?;
    Ok((bytes, content_type, final_url))
}
