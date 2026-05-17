//! Shared application state, registered via `Builder::manage` and injected into
//! commands as `tauri::State<AppState>`.

use reqwest::Client;
use rusqlite::Connection;
use std::sync::RwLock;
use tokio::sync::Mutex;

pub struct AppState {
    /// The single SQLite connection, guarded by an async mutex. All access is
    /// short and synchronous, so the lock is never held across `.await`.
    pub db: Mutex<Connection>,
    /// Shared HTTP client (connection pooling) for all feed fetching. Held
    /// behind an `RwLock` so the network settings (proxy, timeout) can rebuild
    /// it without an app restart. The lock is only ever held to clone the
    /// (cheap, `Arc`-backed) client out — never across an `.await`.
    pub http: RwLock<Client>,
}

impl AppState {
    /// Clone the current HTTP client out for use.
    ///
    /// The lock is never held across an `.await`, but a panic elsewhere could
    /// still poison it. The guarded `Client` is `Arc`-backed and immutable, so
    /// poisoning carries no torn state — recover the guard rather than
    /// propagating the panic and taking the whole backend down.
    pub fn http(&self) -> Client {
        self.http
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Swap in a freshly built HTTP client (e.g. after a proxy/timeout change).
    pub fn set_http(&self, client: Client) {
        *self.http.write().unwrap_or_else(|e| e.into_inner()) = client;
    }
}
