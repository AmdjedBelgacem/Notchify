//! Notification Center database watcher.
//!
//! **Fragile by nature — isolated here.** macOS stores every delivered
//! notification in a private SQLite database owned by `usernoted`:
//!
//! * macOS 15+: `~/Library/Group Containers/group.com.apple.usernoted/db2/db`
//! * macOS ≤ 14: `$(getconf DARWIN_USER_DIR)/com.apple.notificationcenter/db2/db`
//!
//! Reading it requires **Full Disk Access**. The schema is undocumented; we
//! rely only on `record(rec_id, app_id, data, delivered_date)` and
//! `app(app_id, identifier)`, probe for them before querying, and report
//! [`ProviderStatus::Failing`] (never crash) if Apple changes them. `data` is a
//! binary plist whose `req` dictionary holds `titl`, `subt` and `body`.
//!
//! The database is opened read-only and polled by `stat()`ing the db and its
//! WAL every `db_poll_interval_ms` — two syscalls per second at idle.

use notchforge_core::apps::{AppCategory, AppRef};
use notchforge_core::model::{NewNotification, NotificationKind, Origin, Permission, ProviderStatus};
use notchforge_core::provider::{BoxFuture, Provider, ProviderContext, ProviderDescriptor, ProviderError, ProviderEvent, Wake};
use notchforge_core::time::{mac_absolute_to_unix_ms, now_ms};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Bundle id NotchForge itself posts under; never re-ingest our own alerts.
pub const OWN_BUNDLE_ID: &str = "dev.notchforge.NotchForge";

/// Candidate database locations, newest macOS first.
pub fn candidate_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = dirs::home_dir() {
        out.push(home.join("Library/Group Containers/group.com.apple.usernoted/db2/db"));
    }
    if let Some(dir) = darwin_user_dir() {
        out.push(dir.join("com.apple.notificationcenter/db2/db"));
    }
    out
}

#[cfg(target_os = "macos")]
fn darwin_user_dir() -> Option<PathBuf> {
    let mut buf = vec![0 as libc::c_char; 1024];
    // SAFETY: confstr writes at most `len` bytes including the NUL terminator.
    let n = unsafe { libc::confstr(libc::_CS_DARWIN_USER_DIR, buf.as_mut_ptr(), buf.len()) };
    if n == 0 || n > buf.len() {
        return None;
    }
    // SAFETY: buffer is NUL-terminated by confstr.
    let s = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Some(PathBuf::from(s.to_string_lossy().into_owned()))
}

#[cfg(not(target_os = "macos"))]
fn darwin_user_dir() -> Option<PathBuf> {
    None
}

/// Fields decoded from a record's `data` plist.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DecodedRecord {
    pub title: String,
    pub subtitle: Option<String>,
    pub body: String,
    pub thread: Option<String>,
    pub date_unix_ms: Option<i64>,
}

/// Decode the binary (or XML) plist stored in `record.data`.
pub fn decode_record(blob: &[u8]) -> Option<DecodedRecord> {
    let value = plist::Value::from_reader(std::io::Cursor::new(blob)).ok()?;
    let root = value.as_dictionary()?;
    let req = root.get("req").and_then(|v| v.as_dictionary())?;
    let text = |d: &plist::Dictionary, k: &str| {
        d.get(k).and_then(|v| v.as_string()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
    };
    let title = text(req, "titl").unwrap_or_default();
    let body = text(req, "body").unwrap_or_default();
    if title.is_empty() && body.is_empty() {
        return None;
    }
    let date = root.get("date").and_then(|v| v.as_real().or_else(|| v.as_signed_integer().map(|i| i as f64)));
    Some(DecodedRecord {
        title,
        subtitle: text(req, "subt"),
        body,
        thread: text(req, "thre").or_else(|| text(req, "thrd")),
        date_unix_ms: date.map(mac_absolute_to_unix_ms),
    })
}

/// Guess an agent event kind from free text (desktop agents post plain
/// notifications without structured kinds).
pub fn infer_agent_kind(text: &str) -> NotificationKind {
    let t = text.to_lowercase();
    if ["error", "failed", "failure", "crash"].iter().any(|w| t.contains(w)) {
        NotificationKind::Error
    } else if ["permission", "approve", "waiting", "needs your", "input", "confirm", "?"].iter().any(|w| t.contains(w)) {
        NotificationKind::NeedsInput
    } else if ["done", "finished", "complete", "ready", "responded"].iter().any(|w| t.contains(w)) {
        NotificationKind::TaskFinished
    } else {
        NotificationKind::Info
    }
}

pub fn to_notification(bundle_id: &str, rec: DecodedRecord, aliases: &[(String, notchforge_core::apps::KnownApp)]) -> NewNotification {
    let hint = format!("{} {} {}", rec.title, rec.subtitle.as_deref().unwrap_or(""), rec.body);
    let app = AppRef::resolve(bundle_id, &hint, aliases);
    let kind = match app.category() {
        AppCategory::Agent => infer_agent_kind(&hint),
        AppCategory::Messaging => NotificationKind::Message,
        _ => NotificationKind::Info,
    };
    NewNotification {
        app,
        origin: Origin::NotificationCenterDb,
        kind,
        priority: None,
        title: rec.title,
        subtitle: rec.subtitle,
        body: rec.body,
        timestamp_ms: rec.date_unix_ms.unwrap_or_else(now_ms),
        thread: rec.thread,
        url: None,
    }
}

struct Db {
    conn: rusqlite::Connection,
    last_id: i64,
}

fn open(path: &Path) -> Result<Db, ProviderError> {
    // A plain open() distinguishes "no Full Disk Access" (EPERM) from sqlite errors.
    if let Err(e) = std::fs::File::open(path) {
        return Err(if e.kind() == std::io::ErrorKind::PermissionDenied {
            ProviderError::NeedsPermission(Permission::FullDiskAccess)
        } else {
            ProviderError::Io(e)
        });
    }
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = rusqlite::Connection::open_with_flags(path, flags).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("authorization denied") || msg.contains("not authorized") {
            ProviderError::NeedsPermission(Permission::FullDiskAccess)
        } else {
            ProviderError::Unavailable(format!("cannot open notification DB: {msg}"))
        }
    })?;
    conn.busy_timeout(Duration::from_millis(500)).map_err(ProviderError::other)?;
    probe_schema(&conn)?;
    let last_id: i64 = conn
        .query_row("SELECT COALESCE(MAX(rec_id), 0) FROM record", [], |r| r.get(0))
        .map_err(|e| ProviderError::Unavailable(format!("notification DB query failed: {e}")))?;
    Ok(Db { conn, last_id })
}

fn probe_schema(conn: &rusqlite::Connection) -> Result<(), ProviderError> {
    let columns = |table: &str| -> Result<Vec<String>, ProviderError> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|e| ProviderError::Unavailable(format!("schema probe failed: {e}")))?;
        let names = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(|e| ProviderError::Unavailable(format!("schema probe failed: {e}")))?
            .filter_map(Result::ok)
            .collect();
        Ok(names)
    };
    let record = columns("record")?;
    let app = columns("app")?;
    let need = |cols: &[String], name: &str| cols.iter().any(|c| c == name);
    if !(need(&record, "rec_id") && need(&record, "app_id") && need(&record, "data") && need(&app, "identifier")) {
        return Err(ProviderError::Unavailable(
            "unsupported Notification Center DB schema (macOS changed it); use the Accessibility fallback".into(),
        ));
    }
    Ok(())
}

/// Fetch records newer than `db.last_id`.
fn fetch_new(db: &mut Db) -> Result<Vec<(String, Vec<u8>)>, ProviderError> {
    let mut stmt = db
        .conn
        .prepare_cached(
            "SELECT r.rec_id, a.identifier, r.data FROM record r JOIN app a ON a.app_id = r.app_id \
             WHERE r.rec_id > ?1 ORDER BY r.rec_id ASC LIMIT 200",
        )
        .map_err(ProviderError::other)?;
    let rows = stmt
        .query_map([db.last_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?)))
        .map_err(ProviderError::other)?;
    let mut out = Vec::new();
    for row in rows {
        let (id, bundle, data) = row.map_err(ProviderError::other)?;
        db.last_id = db.last_id.max(id);
        out.push((bundle, data));
    }
    Ok(out)
}

fn mtimes(path: &Path) -> (Option<SystemTime>, Option<SystemTime>) {
    let m = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    (m(path), m(Path::new(&wal)))
}

pub struct UsernotedProvider;

impl Provider for UsernotedProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: "notification_center",
            name: "Notification Center",
            permissions: &[Permission::FullDiskAccess],
        }
    }

    fn run(self: Box<Self>, mut ctx: ProviderContext) -> BoxFuture<'static, Result<(), ProviderError>> {
        Box::pin(async move {
            if !cfg!(target_os = "macos") {
                ctx.set_status(ProviderStatus::Failing("Notification Center needs macOS".into())).await;
                return Ok(());
            }
            let candidates = candidate_paths();
            let Some(path) = candidates.iter().find(|p| p.exists() || p.parent().is_some_and(Path::exists)).cloned()
            else {
                // Without FDA even `exists()` on the group container can fail.
                return Err(ProviderError::NeedsPermission(Permission::FullDiskAccess));
            };
            let p = path.clone();
            let mut db = tokio::task::spawn_blocking(move || open(&p)).await.map_err(ProviderError::other)??;
            tracing::info!(path = %path.display(), last_id = db.last_id, "watching Notification Center DB");
            ctx.set_status(ProviderStatus::Running).await;

            let mut seen = mtimes(&path);
            loop {
                let interval = Duration::from_millis(ctx.config().notifications.db_poll_interval_ms);
                if ctx.sleep(interval).await == Wake::Shutdown {
                    return Ok(());
                }
                let now = mtimes(&path);
                if now == seen {
                    continue;
                }
                seen = now;
                let (returned, rows) = tokio::task::spawn_blocking(move || {
                    let rows = fetch_new(&mut db);
                    (db, rows)
                })
                .await
                .map_err(ProviderError::other)?;
                db = returned;
                let aliases = ctx.config().notifications.alias_pairs();
                for (bundle, data) in rows? {
                    if bundle == OWN_BUNDLE_ID {
                        continue;
                    }
                    let Some(rec) = decode_record(&data) else { continue };
                    let n = to_notification(&bundle, rec, &aliases);
                    if !ctx.emit(ProviderEvent::Notification(n)).await {
                        return Ok(());
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notchforge_core::apps::KnownApp;

    fn blob(title: &str, body: &str) -> Vec<u8> {
        let mut req = plist::Dictionary::new();
        req.insert("titl".into(), title.into());
        req.insert("subt".into(), "#general".into());
        req.insert("body".into(), body.into());
        let mut root = plist::Dictionary::new();
        root.insert("app".into(), "com.hnc.Discord".into());
        root.insert("date".into(), plist::Value::Real(0.0));
        root.insert("req".into(), plist::Value::Dictionary(req));
        let mut out = Vec::new();
        plist::to_writer_binary(&mut out, &plist::Value::Dictionary(root)).expect("encode");
        out
    }

    #[test]
    fn decodes_binary_plist_records() {
        let rec = decode_record(&blob("Alice", "hello there")).expect("decode");
        assert_eq!(rec.title, "Alice");
        assert_eq!(rec.subtitle.as_deref(), Some("#general"));
        assert_eq!(rec.body, "hello there");
        assert_eq!(rec.date_unix_ms, Some(978_307_200_000));
        assert!(decode_record(&blob("", "")).is_none());
        assert!(decode_record(b"junk").is_none());
    }

    #[test]
    fn maps_to_notification() {
        let rec = decode_record(&blob("Alice", "hello")).expect("decode");
        let n = to_notification("com.hnc.Discord", rec, &[]);
        assert_eq!(n.app.known, Some(KnownApp::Discord));
        assert_eq!(n.kind, NotificationKind::Message);
        assert_eq!(n.origin, Origin::NotificationCenterDb);

        let rec = DecodedRecord { title: "Claude".into(), body: "Claude needs your permission".into(), ..Default::default() };
        let n = to_notification("com.anthropic.claudefordesktop", rec, &[]);
        assert_eq!(n.kind, NotificationKind::NeedsInput);
    }

    #[test]
    fn agent_kind_inference() {
        assert_eq!(infer_agent_kind("Build failed"), NotificationKind::Error);
        assert_eq!(infer_agent_kind("Task complete"), NotificationKind::TaskFinished);
        assert_eq!(infer_agent_kind("hello"), NotificationKind::Info);
    }

    #[test]
    fn reads_a_db_with_the_expected_schema() {
        let path = std::env::temp_dir().join(format!("nf-usernoted-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let c = rusqlite::Connection::open(&path).expect("create");
            c.execute_batch(
                "CREATE TABLE app (app_id INTEGER PRIMARY KEY, identifier TEXT);
                 CREATE TABLE record (rec_id INTEGER PRIMARY KEY, app_id INTEGER, data BLOB, delivered_date REAL);
                 INSERT INTO app VALUES (1, 'com.hnc.Discord');",
            )
            .expect("schema");
            c.execute("INSERT INTO record (app_id, data) VALUES (1, ?1)", [blob("old", "x")]).expect("insert");
        }
        let mut db = open(&path).expect("open");
        assert_eq!(db.last_id, 1, "history is not replayed");
        {
            let c = rusqlite::Connection::open(&path).expect("writer");
            c.execute("INSERT INTO record (app_id, data) VALUES (1, ?1)", [blob("new", "y")]).expect("insert");
        }
        let rows = fetch_new(&mut db).expect("fetch");
        assert_eq!(rows.len(), 1);
        assert_eq!(decode_record(&rows[0].1).expect("decode").title, "new");
        assert!(fetch_new(&mut db).expect("fetch").is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_unknown_schema() {
        let path = std::env::temp_dir().join(format!("nf-usernoted-bad-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        rusqlite::Connection::open(&path).expect("create").execute_batch("CREATE TABLE other (x);").expect("schema");
        assert!(matches!(open(&path), Err(ProviderError::Unavailable(_))));
        let _ = std::fs::remove_file(&path);
    }
}
