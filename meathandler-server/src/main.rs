//! MeatHandler telemetry receiver.
//!
//! The WiCAN Pro "HTTPS" automate destination POSTs JSON here. The first
//! POST after the dongle boots carries `autopid_data`, `config` and `status`;
//! every later POST carries only `autopid_data` (flat name -> value map).
//!
//! Two listeners share one router:
//! - HTTPS (default :8443) for the dongle, which refuses plain http:// URLs.
//! - HTTP  (default :8080) for the LAN dashboard and curl testing.
//!
//! Ingest needs `Authorization: Bearer $MH_TOKEN`. The dashboard and read
//! API need HTTP Basic auth when `MH_DASH_PASS` is set.
//!
//! TLS picks a certificate by SNI: clients asking for `MH_PUBLIC_HOST`
//! (meathandler.ddns.net) get the public No-IP certificate; anything else,
//! including connections straight to the LAN IP, gets the local-CA one.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Bytes,
    extract::{Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use rustls::{
    pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer},
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
    ServerConfig,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tracing_subscriber::EnvFilter;

mod charge_watch;

const SCHEMA: &str = "
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS samples (ts INTEGER NOT NULL, key TEXT NOT NULL, value REAL NOT NULL);
CREATE INDEX IF NOT EXISTS samples_key_ts ON samples(key, ts);
CREATE TABLE IF NOT EXISTS latest (key TEXT PRIMARY KEY, ts INTEGER NOT NULL, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS device (id INTEGER PRIMARY KEY CHECK (id = 1), ts INTEGER NOT NULL, status TEXT, config TEXT);
CREATE TABLE IF NOT EXISTS posts (ts INTEGER NOT NULL, bytes INTEGER NOT NULL);
";

const DASHBOARD: &str = include_str!("dashboard.html");

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) db: Arc<Mutex<Connection>>,
    token: Option<String>,
    /// Expected `user:pass` for the dashboard, if read auth is on.
    dash_auth: Option<String>,
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    let _ = rustls::crypto::ring::default_provider().install_default();

    let db_path = env_or("MH_DB", "meathandler.db");
    let conn = Connection::open(&db_path)?;
    conn.execute_batch(SCHEMA)?;

    let token = std::env::var("MH_TOKEN").ok().filter(|t| !t.trim().is_empty());
    if token.is_none() {
        tracing::warn!("MH_TOKEN is not set, ingest accepts anyone on the network");
    }
    let dash_auth = std::env::var("MH_DASH_PASS")
        .ok()
        .filter(|p| !p.is_empty())
        .map(|p| format!("{}:{}", env_or("MH_DASH_USER", "enoch"), p));
    if dash_auth.is_none() {
        tracing::warn!("MH_DASH_PASS is not set, dashboard is readable by anyone who can reach it");
    }
    let state = AppState { db: Arc::new(Mutex::new(conn)), token, dash_auth };
    match charge_watch::Config::from_env() {
        Some(cfg) => {
            tokio::spawn(charge_watch::run(state.clone(), cfg));
        }
        None => tracing::info!("MH_NTFY_TOPIC not set, charge alerts off"),
    }

    // The Slint dashboard compiled to wasm (meatpi repo: deploy/build-web.sh).
    // Gzip matters: the .wasm is ~11 MB raw, ~5 MB compressed.
    let web_dir = env_or("MH_WEB_DIR", "/usr/local/share/meathandler/web");
    let web_app = tower::ServiceBuilder::new()
        .layer(tower_http::compression::CompressionLayer::new())
        .service(tower_http::services::ServeDir::new(&web_dir));

    let reads = Router::new()
        .route("/", get(|| async { Html(DASHBOARD) }))
        .route("/api/latest", get(latest))
        .route("/api/history", get(history))
        .route("/api/device", get(device))
        .nest_service("/app", web_app)
        .layer(middleware::from_fn_with_state(state.clone(), require_dash_auth));
    let app = Router::new()
        .merge(reads)
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/wican", post(ingest))
        .with_state(state);

    let http_addr: SocketAddr = env_or("MH_HTTP_ADDR", "0.0.0.0:8080").parse()?;
    let https_addr: SocketAddr = env_or("MH_HTTPS_ADDR", "0.0.0.0:8443").parse()?;
    let tls = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(tls_config()?));

    tracing::info!(%http_addr, %https_addr, %db_path, "meathandler listening");
    let http = axum_server::bind(http_addr).serve(app.clone().into_make_service());
    let https = axum_server::bind_rustls(https_addr, tls).serve(app.into_make_service());
    tokio::try_join!(http, https)?;
    Ok(())
}

fn load_certified_key(cert_path: &str, key_path: &str) -> anyhow::Result<Arc<CertifiedKey>> {
    let certs = CertificateDer::pem_file_iter(cert_path)
        .map_err(|e| anyhow::anyhow!("{cert_path}: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("{cert_path}: {e}"))?;
    anyhow::ensure!(!certs.is_empty(), "{cert_path}: no certificates found");
    let key = PrivateKeyDer::from_pem_file(key_path).map_err(|e| anyhow::anyhow!("{key_path}: {e}"))?;
    let signer = rustls::crypto::ring::sign::any_supported_type(&key)?;
    Ok(Arc::new(CertifiedKey::new(certs, signer)))
}

#[derive(Debug)]
struct SniResolver {
    local: Arc<CertifiedKey>,
    public: Option<(String, Arc<CertifiedKey>)>,
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        if let (Some(name), Some((host, key))) = (hello.server_name(), &self.public) {
            if name.eq_ignore_ascii_case(host) {
                return Some(key.clone());
            }
        }
        Some(self.local.clone())
    }
}

fn tls_config() -> anyhow::Result<ServerConfig> {
    let local = load_certified_key(
        &env_or("MH_TLS_CERT", "certs/server.crt"),
        &env_or("MH_TLS_KEY", "certs/server.key"),
    )?;
    let public = match (std::env::var("MH_PUBLIC_CERT"), std::env::var("MH_PUBLIC_KEY")) {
        (Ok(c), Ok(k)) if !c.is_empty() && !k.is_empty() => {
            let host = env_or("MH_PUBLIC_HOST", "meathandler.ddns.net");
            tracing::info!(%host, "public certificate loaded");
            Some((host, load_certified_key(&c, &k)?))
        }
        _ => None,
    };
    let mut cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(SniResolver { local, public }));
    cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(cfg)
}

async fn require_dash_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(expected) = &state.dash_auth else { return next.run(req).await };
    let given = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b.trim()).ok())
        .and_then(|b| String::from_utf8(b).ok());
    if given.as_deref() == Some(expected.as_str()) {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"MeatHandler\"")],
            "login required",
        )
            .into_response()
    }
}

fn authorized(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(expected) = &state.token else { return true };
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim() == expected)
        .unwrap_or(false)
}

/// Blank out anything that looks like a credential. The boot-time `status`
/// block includes Wi-Fi passwords and tokens, which we never want on disk.
fn redact(v: Value) -> Value {
    const SECRET_HINTS: [&str; 5] = ["pass", "token", "secret", "pwd", "key"];
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .map(|(k, x)| {
                    let lower = k.to_lowercase();
                    let leaf = !(x.is_object() || x.is_array());
                    if leaf && SECRET_HINTS.iter().any(|h| lower.contains(h)) {
                        (k, Value::String("***".into()))
                    } else {
                        (k, redact(x))
                    }
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(redact).collect()),
        other => other,
    }
}

fn store(db: &mut Connection, payload: &Value, bytes: usize) -> rusqlite::Result<usize> {
    let ts = now_ms();
    let tx = db.transaction()?;
    let mut count = 0;

    // Docs show `autopid_data`; one section also shows `metrics`. Accept both.
    let data = payload.get("autopid_data").or_else(|| payload.get("metrics"));
    if let Some(Value::Object(values)) = data {
        for (name, val) in values {
            tx.execute(
                "INSERT INTO latest(key, ts, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET ts = excluded.ts, value = excluded.value",
                params![name, ts, val.to_string()],
            )?;
            if let Some(f) = val.as_f64() {
                tx.execute("INSERT INTO samples(ts, key, value) VALUES (?1, ?2, ?3)", params![ts, name, f])?;
            }
            count += 1;
        }
    }

    let status = payload.get("status").map(|x| redact(x.clone()).to_string());
    let config = payload.get("config").map(|x| redact(x.clone()).to_string());
    if status.is_some() || config.is_some() {
        tx.execute(
            "INSERT INTO device(id, ts, status, config) VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET ts = excluded.ts,
               status = COALESCE(excluded.status, device.status),
               config = COALESCE(excluded.config, device.config)",
            params![ts, status, config],
        )?;
    }

    tx.execute("INSERT INTO posts(ts, bytes) VALUES (?1, ?2)", params![ts, bytes as i64])?;
    tx.commit()?;
    Ok(count)
}

async fn ingest(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if !authorized(&state, &headers) {
        tracing::warn!("rejected ingest: missing or wrong bearer token");
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("rejected ingest: bad JSON ({e})");
            return (StatusCode::BAD_REQUEST, "bad json").into_response();
        }
    };
    let result = {
        let mut db = state.db.lock().unwrap_or_else(|p| p.into_inner());
        store(&mut db, &payload, body.len())
    };
    match result {
        Ok(n) => {
            tracing::info!(values = n, bytes = body.len(), "ingest");
            Json(json!({ "ok": true, "values": n })).into_response()
        }
        Err(e) => {
            tracing::error!("db error on ingest: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response()
        }
    }
}

fn db_response(result: rusqlite::Result<Value>) -> Response {
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => {
            tracing::error!("db error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "db error").into_response()
        }
    }
}

async fn latest(State(state): State<AppState>) -> Response {
    let db = state.db.lock().unwrap_or_else(|p| p.into_inner());
    db_response((|| {
        let mut stmt = db.prepare("SELECT key, ts, value FROM latest ORDER BY key")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))?;
        let mut values = Map::new();
        for row in rows {
            let (k, ts, raw) = row?;
            let v: Value = serde_json::from_str(&raw).unwrap_or(Value::String(raw));
            values.insert(k, json!({ "ts": ts, "value": v }));
        }
        let last_post: Option<i64> = db.query_row("SELECT MAX(ts) FROM posts", [], |r| r.get(0))?;
        let posts_24h: i64 = db.query_row(
            "SELECT COUNT(*) FROM posts WHERE ts > ?1",
            params![now_ms() - 86_400_000],
            |r| r.get(0),
        )?;
        Ok(json!({ "now": now_ms(), "last_post": last_post, "posts_24h": posts_24h, "values": values }))
    })())
}

#[derive(Deserialize)]
struct HistoryParams {
    key: String,
    hours: Option<f64>,
    limit: Option<i64>,
}

async fn history(State(state): State<AppState>, Query(q): Query<HistoryParams>) -> Response {
    let since = now_ms() - (q.hours.unwrap_or(24.0) * 3_600_000.0) as i64;
    let limit = q.limit.unwrap_or(5000).clamp(1, 100_000);
    let db = state.db.lock().unwrap_or_else(|p| p.into_inner());
    db_response((|| {
        let mut stmt = db.prepare(
            "SELECT ts, value FROM samples WHERE key = ?1 AND ts > ?2 ORDER BY ts DESC LIMIT ?3",
        )?;
        let mut points: Vec<Value> = stmt
            .query_map(params![q.key, since, limit], |r| Ok(json!([r.get::<_, i64>(0)?, r.get::<_, f64>(1)?])))?
            .collect::<Result<_, _>>()?;
        points.reverse();
        Ok(json!({ "key": q.key, "points": points }))
    })())
}

async fn device(State(state): State<AppState>) -> Response {
    let db = state.db.lock().unwrap_or_else(|p| p.into_inner());
    db_response((|| {
        let row = db
            .query_row("SELECT ts, status, config FROM device WHERE id = 1", [], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?))
            })
            .optional()?;
        let parse = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null);
        Ok(match row {
            Some((ts, status, config)) => json!({ "ts": ts, "status": parse(status), "config": parse(config) }),
            None => json!({ "ts": null, "status": null, "config": null }),
        })
    })())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SCHEMA).unwrap();
        c
    }

    #[test]
    fn stores_boot_and_periodic_payloads() {
        let mut db = mem();
        let boot = json!({
            "autopid_data": { "SOC": 81.5, "SPEED": 0 },
            "config": { "SOC": { "class": "battery", "unit": "%" } },
            "status": { "device_id": "14c19fd9573d", "sta_pass": "hunter2", "sta_ip": "10.43.1.39" }
        });
        assert_eq!(store(&mut db, &boot, 10).unwrap(), 2);
        assert_eq!(store(&mut db, &json!({ "autopid_data": { "SOC": 81.0 } }), 10).unwrap(), 1);

        let soc: String = db.query_row("SELECT value FROM latest WHERE key = 'SOC'", [], |r| r.get(0)).unwrap();
        assert_eq!(soc, "81.0");
        let n: i64 = db.query_row("SELECT COUNT(*) FROM samples WHERE key = 'SOC'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
        let status: String = db.query_row("SELECT status FROM device", [], |r| r.get(0)).unwrap();
        assert!(status.contains("10.43.1.39") && !status.contains("hunter2"));
    }
}
