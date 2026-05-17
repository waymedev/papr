//! FreshRSS synchronisation over the Google Reader compatible API.
//!
//! Flow: `ClientLogin` for an auth token, push any queued local read/starred
//! changes via `edit-tag`, then pull the subscription list (to subscribe to
//! new feeds) and the recent reading-list (to reconcile read/starred state,
//! matched to local articles by URL).

use crate::db;
use crate::error::{AppError, AppResult};
use crate::ingestion::parse;
use crate::state::AppState;
use reqwest::{Client, RequestBuilder};
use serde::Deserialize;
use tauri::{AppHandle, Manager};

const READ_TAG: &str = "user/-/state/com.google/read";
const STARRED_TAG: &str = "user/-/state/com.google/starred";
const READING_LIST: &str = "user/-/state/com.google/reading-list";

/// Normalise a user-supplied FreshRSS URL to its GReader API root.
fn greader_base(url: &str) -> String {
    let t = url.trim().trim_end_matches('/');
    if t.contains("/api/greader.php") {
        t.to_string()
    } else {
        format!("{t}/api/greader.php")
    }
}

/// An authenticated FreshRSS session.
struct Session {
    base: String,
    auth: String,
    token: String,
}

impl Session {
    fn get(&self, http: &Client, path: &str) -> RequestBuilder {
        http.get(format!("{}/reader/api/0/{path}", self.base))
            .header("Authorization", format!("GoogleLogin auth={}", self.auth))
    }
    fn post(&self, http: &Client, path: &str) -> RequestBuilder {
        http.post(format!("{}/reader/api/0/{path}", self.base))
            .header("Authorization", format!("GoogleLogin auth={}", self.auth))
    }
}

/// Exchange username + password for a long-lived auth token.
async fn client_login(http: &Client, base: &str, user: &str, pass: &str) -> AppResult<String> {
    let resp = http
        .post(format!("{base}/accounts/ClientLogin"))
        .form(&[("Email", user), ("Passwd", pass)])
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(AppError::code("freshrssLoginFailed"));
    }
    let body = resp.text().await?;
    body.lines()
        .find_map(|l| l.strip_prefix("Auth="))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::code("freshrssNoToken"))
}

/// Log in and obtain both the auth token and a write (edit-tag) token.
async fn login(http: &Client, base: &str, user: &str, pass: &str) -> AppResult<Session> {
    let auth = client_login(http, base, user, pass).await?;
    let token = http
        .get(format!("{base}/reader/api/0/token"))
        .header("Authorization", format!("GoogleLogin auth={auth}"))
        .send()
        .await?
        .text()
        .await?
        .trim()
        .to_string();
    Ok(Session {
        base: base.to_string(),
        auth,
        token,
    })
}

#[derive(Deserialize)]
struct SubList {
    #[serde(default)]
    subscriptions: Vec<Sub>,
}
#[derive(Deserialize)]
struct Sub {
    url: Option<String>,
    title: Option<String>,
}

#[derive(Deserialize)]
struct Contents {
    #[serde(default)]
    items: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    id: String,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    canonical: Vec<Href>,
    #[serde(default)]
    alternate: Vec<Href>,
}
#[derive(Deserialize)]
struct Href {
    href: String,
}

/// Stored FreshRSS credentials, if a server is configured.
async fn creds(app: &AppHandle) -> AppResult<Option<(String, String, String)>> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().await;
    let url = db::get_setting(&conn, "freshrss_url")?;
    let user = db::get_setting(&conn, "freshrss_user")?;
    let pass = db::get_setting(&conn, "freshrss_pass")?;
    Ok(match (url, user, pass) {
        (Some(u), Some(usr), Some(p)) if !u.trim().is_empty() && !usr.is_empty() => {
            Some((u, usr, p))
        }
        _ => None,
    })
}

/// The configured FreshRSS server URL, or `None` when not connected.
pub async fn connected_url(app: &AppHandle) -> AppResult<Option<String>> {
    Ok(creds(app).await?.map(|(u, _, _)| u))
}

/// Verify credentials against the server and, on success, persist them.
pub async fn connect(app: &AppHandle, url: &str, user: &str, pass: &str) -> AppResult<()> {
    let base = greader_base(url);
    let http = app.state::<AppState>().http();
    login(&http, &base, user, pass).await?; // verifies credentials

    let state = app.state::<AppState>();
    let conn = state.db.lock().await;
    db::set_setting(&conn, "freshrss_url", url.trim())?;
    db::set_setting(&conn, "freshrss_user", user)?;
    db::set_setting(&conn, "freshrss_pass", pass)?;
    Ok(())
}

/// Forget the stored FreshRSS credentials.
pub async fn disconnect(app: &AppHandle) -> AppResult<()> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().await;
    for key in ["freshrss_url", "freshrss_user", "freshrss_pass"] {
        db::set_setting(&conn, key, "")?;
    }
    Ok(())
}

/// Run a full sync if a server is connected; otherwise a no-op.
pub async fn run_if_connected(app: &AppHandle) -> AppResult<()> {
    if creds(app).await?.is_some() {
        sync_now(app).await.map(|_| ())
    } else {
        Ok(())
    }
}

/// Push queued changes, then pull subscriptions and read/starred state.
/// Returns the number of local articles whose state was reconciled.
pub async fn sync_now(app: &AppHandle) -> AppResult<usize> {
    let (url, user, pass) = creds(app)
        .await?
        .ok_or_else(|| AppError::code("freshrssNotConnected"))?;
    let base = greader_base(&url);
    let http = app.state::<AppState>().http();
    let session = login(&http, &base, &user, &pass).await?;

    // 1 ── push: flush queued local read/starred changes. `take_sync_queue`
    // removes the rows up front, so any push that fails must be re-queued —
    // otherwise a network blip silently drops the user's change forever.
    let queue = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().await;
        db::take_sync_queue(&conn)?
    };
    let mut failed: Vec<db::SyncEntry> = Vec::new();
    for entry in queue {
        let tag = if entry.field == "starred" {
            STARRED_TAG
        } else {
            READ_TAG
        };
        let action = if entry.value { "a" } else { "r" };
        let pushed = session
            .post(&http, "edit-tag")
            .form(&[
                ("i", entry.remote_id.as_str()),
                (action, tag),
                ("T", session.token.as_str()),
            ])
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .is_ok();
        if !pushed {
            failed.push(entry);
        }
    }
    if !failed.is_empty() {
        log::warn!("sync: {} change(s) failed to push, re-queued", failed.len());
        let state = app.state::<AppState>();
        let conn = state.db.lock().await;
        for entry in &failed {
            let _ = db::requeue_sync(&conn, entry.article_id, &entry.field, entry.value);
        }
    }

    // 2 ── pull subscriptions: subscribe locally to any feed we don't have.
    let subs: SubList = session
        .get(&http, "subscription/list?output=json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    {
        let state = app.state::<AppState>();
        let conn = state.db.lock().await;
        for sub in subs.subscriptions {
            let Some(feed_url) = sub.url.filter(|u| !u.is_empty()) else {
                continue;
            };
            if db::find_feed_by_url(&conn, &feed_url)?.is_none() {
                let title = sub.title.unwrap_or_else(|| feed_url.clone());
                let st = parse::detect_source_type(&feed_url);
                let _ = db::insert_feed(&conn, &feed_url, None, &title, None, st, None);
            }
        }
    }

    // 3 ── pull read/starred state for recent items, matched by URL.
    let contents: Contents = session
        .get(
            &http,
            &format!("stream/contents/{READING_LIST}?output=json&n=1000"),
        )
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let mut reconciled = 0usize;
    {
        let state = app.state::<AppState>();
        let conn = state.db.lock().await;
        // Articles with still-unsent local edits keep their local state; we
        // only assign their remote id so the next sync can push them.
        let pending: std::collections::HashSet<i64> =
            db::pending_sync_article_ids(&conn)?.into_iter().collect();
        for item in contents.items {
            let url = item
                .canonical
                .first()
                .or_else(|| item.alternate.first())
                .map(|h| h.href.clone());
            let Some(url) = url else { continue };
            if let Some(aid) = db::article_id_by_url(&conn, &url)? {
                db::set_remote_id(&conn, aid, &item.id)?;
                if !pending.contains(&aid) {
                    let read = item.categories.iter().any(|c| c == READ_TAG);
                    let starred = item.categories.iter().any(|c| c == STARRED_TAG);
                    db::set_sync_state(&conn, aid, read, starred)?;
                }
                reconciled += 1;
            }
        }
    }
    Ok(reconciled)
}
