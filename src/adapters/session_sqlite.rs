//! Read-only views of native shared databases. No provider-history mutations.
use super::{validate_provider_id, StoredSession};
use crate::domain::Provider;
use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

pub(super) const MASTRA_READY: &str = "info & shortcuts";
// The welcome banner is printed before prompt_toolkit enters raw mode. Wait
// for its actual editor prompt too, or the task's Enter is consumed at startup.
pub(super) const HERMES_READY: &str = "Type your message or /help for commands.\n❯";

pub(super) fn supports(provider: &Provider) -> bool {
    matches!(
        provider,
        Provider::Hermes | Provider::MastraCode | Provider::Devin
    )
}

pub(super) fn prompt_input(prompt: &str) -> Vec<u8> {
    // Bracketed paste keeps multiline text and slash-prefixed tasks in the
    // native editor, rather than interpreting embedded newlines as commands.
    format!("\x1b[200~{}\x1b[201~\r", prompt.trim()).into_bytes()
}

pub(super) fn default_path(provider: &Provider) -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("home directory is not set")?;
    match provider {
        Provider::Hermes => Ok(std::env::var_os("HERMES_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".hermes"))
            .join("state.db")),
        Provider::MastraCode => Ok(std::env::var_os("MASTRA_DB_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("MASTRA_APP_DATA_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| app_data(&home).join("mastracode"))
                    .join("mastra.db")
            })),
        Provider::Devin => Ok(app_data(&home).join("devin/cli/sessions.db")),
        _ => bail!("not a SQLite harness"),
    }
}

pub(super) fn require_local_store(provider: &Provider) -> Result<()> {
    if *provider == Provider::MastraCode
        && (std::env::var_os("MASTRA_DB_URL").is_some()
            || std::env::var("MASTRA_STORAGE_BACKEND").is_ok_and(|v| v == "pg"))
    {
        bail!("OAV supports MastraCode's local SQLite store, not remote LibSQL/PostgreSQL");
    }
    Ok(())
}

fn app_data(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Roaming"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
    }
}

fn open(path: &Path) -> Result<Option<Connection>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > 2 * 1024 * 1024 * 1024
    {
        bail!("native database must be a regular file smaller than 2 GiB");
    }
    // Reject redirects at the database and sidecars. System parent aliases
    // (notably /tmp on macOS) are legitimate and are not provider-owned files.
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        if fs::symlink_metadata(PathBuf::from(name)).is_ok_and(|m| m.file_type().is_symlink()) {
            bail!("native database sidecar must not be a symlink");
        }
    }
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(Duration::from_millis(150))?;
    db.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
    let deadline = Instant::now() + Duration::from_millis(500);
    db.progress_handler(10_000, Some(move || Instant::now() > deadline));
    Ok(Some(db))
}

pub(super) fn list(
    provider: &Provider,
    path: &Path,
    id: Option<&str>,
    limit: usize,
) -> Result<Vec<StoredSession>> {
    if let Some(id) = id {
        validate_provider_id(provider, id)?;
    }
    let Some(db) = open(path)? else {
        return Ok(Vec::new());
    };
    // One SELECT gives a consistent snapshot including committed WAL frames.
    // Owned refreshes use the primary key. Only external discovery orders the
    // complete inventory; message bodies are read for the selected rows only.
    let (table, hidden, order, columns) = match provider {
        Provider::Hermes => ("sessions", "COALESCE(s.hidden,0)=0 AND COALESCE(s.archived,0)=0", "COALESCE(s.last_activity_at,s.started_at)",
            "s.id, COALESCE(s.cwd,''), COALESCE(s.title,''), s.model, s.started_at,
             MAX(COALESCE(s.last_activity_at,0),COALESCE(s.ended_at,0),s.started_at,
                 COALESCE((SELECT MAX(timestamp) FROM messages WHERE session_id=s.id AND active=1),0)),
             COALESCE((SELECT CASE WHEN substr(content,1,1)='[' AND json_valid(content)
                       THEN (SELECT substr(group_concat(json_extract(p.value,'$.text'),' '),1,8192) FROM json_each(content) p WHERE json_extract(p.value,'$.type')='text')
                       ELSE substr(content,1,8192) END FROM messages WHERE session_id=s.id AND active=1
                       AND role IN ('user','assistant') AND content IS NOT NULL AND content != '' ORDER BY id DESC LIMIT 1),'')"),
        Provider::MastraCode => ("mastra_threads", "1=1", "s.updatedAt",
            "s.id, COALESCE(json_extract(s.metadata,'$.projectPath'),''), s.title,
             json_extract(s.metadata,'$.currentModelId'), CAST(strftime('%s',s.createdAt) AS REAL),
             MAX(COALESCE(CAST(strftime('%s',s.updatedAt) AS REAL),0),
                 COALESCE((SELECT MAX(CAST(strftime('%s',createdAt) AS REAL)) FROM mastra_messages WHERE thread_id=s.id),0)),
             COALESCE((SELECT (SELECT substr(group_concat(json_extract(p.value,'$.text'),' '),1,8192)
                      FROM json_each(m.content,'$.parts') p WHERE json_extract(p.value,'$.type')='text')
                      FROM mastra_messages m WHERE thread_id=s.id AND json_valid(m.content)
                      AND role IN ('user','assistant','signal') AND EXISTS(SELECT 1 FROM json_each(m.content,'$.parts') p WHERE json_extract(p.value,'$.type')='text')
                      ORDER BY createdAt DESC,rowid DESC LIMIT 1),'')"),
        Provider::Devin => ("sessions", "s.hidden=0", "s.last_activity_at",
            "s.id, s.working_directory, COALESCE(s.title,''), s.model, s.created_at, s.last_activity_at,
             COALESCE((WITH RECURSIVE chain(node_id,parent_node_id,chat_message,depth) AS (
                 SELECT node_id,parent_node_id,chat_message,0 FROM message_nodes WHERE session_id=s.id AND node_id=s.main_chain_id
                 UNION ALL SELECT n.node_id,n.parent_node_id,n.chat_message,c.depth+1 FROM message_nodes n JOIN chain c
                 ON n.node_id=c.parent_node_id WHERE n.session_id=s.id AND c.depth<64
             ) SELECT substr(json_extract(chat_message,'$.content'),1,8192) FROM chain WHERE json_extract(chat_message,'$.role') IN ('user','assistant')
                 AND COALESCE(json_extract(chat_message,'$.content'),'') != '' ORDER BY depth LIMIT 1),'')"),
        _ => bail!("not a SQLite harness"),
    };
    let filter = if id.is_some() {
        "AND s.id=?1"
    } else {
        "AND ?1 IS NULL"
    };
    let sql = format!(
        "SELECT {columns} FROM {table} s WHERE {hidden} {filter} ORDER BY {order} DESC LIMIT ?2"
    );
    let mut statement = db
        .prepare(&sql)
        .with_context(|| format!("unsupported {} session database schema", provider.label()))?;
    let mut rows = statement.query(rusqlite::params![id, limit.min(10_000) as i64])?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next()? {
        let session_id: String = row.get(0)?;
        if validate_provider_id(provider, &session_id).is_err() {
            continue;
        }
        let cwd: String = row.get(1)?;
        if !cwd.is_empty() && !Path::new(&cwd).is_absolute() {
            continue;
        }
        let summary: String = row.get(6)?;
        sessions.push(StoredSession {
            session_id,
            cwd: PathBuf::from(cwd),
            name: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            summary,
            model: row.get(3)?,
            created_at: timestamp(row.get(4)?),
            updated_at: timestamp(row.get(5)?),
            path: Some(path.to_owned()),
        });
    }
    Ok(sessions)
}

fn timestamp(seconds: Option<f64>) -> Option<std::time::SystemTime> {
    seconds
        .filter(|s| s.is_finite() && *s > 0.0 && *s < 253_402_300_800.0)
        .and_then(|s| UNIX_EPOCH.checked_add(Duration::from_secs_f64(s)))
}

pub(super) fn mastra_resource(path: &Path, id: &str) -> Result<String> {
    validate_provider_id(&Provider::MastraCode, id)?;
    let db = open(path)?.context("MastraCode session database is missing")?;
    let resource: String = db.query_row(
        "SELECT resourceId FROM mastra_threads WHERE id=?1",
        [id],
        |row| row.get(0),
    )?;
    if !super::valid_text(&resource) {
        bail!("invalid MastraCode resource ID");
    }
    Ok(resource)
}

pub(super) fn saved_models(provider: &Provider, path: &Path) -> Result<Vec<String>> {
    let Some(db) = open(path)? else {
        return Ok(Vec::new());
    };
    let sql = match provider {
        Provider::Hermes => "SELECT DISTINCT model FROM sessions WHERE model IS NOT NULL LIMIT 256",
        Provider::MastraCode => "SELECT DISTINCT json_extract(metadata,'$.currentModelId') FROM mastra_threads LIMIT 256",
        _ => bail!("not a saved-model provider"),
    };
    let mut statement = db.prepare(sql)?;
    let mut models = statement
        .query_map([], |row| row.get::<_, Option<String>>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .filter(|m| super::valid_text(m))
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    Ok(models)
}

pub(super) fn devin_models(input: &str) -> Result<Vec<String>> {
    let value: Value = serde_json::from_str(input).context("invalid Devin model list")?;
    let entries = value
        .as_array()
        .or_else(|| value["models"].as_array())
        .context("Devin models must be an array")?;
    let mut models = entries
        .iter()
        .filter_map(|entry| entry.as_str().or_else(|| entry["id"].as_str()))
        .filter(|id| super::valid_text(id))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::session_migrate_native::{
        SessionMigrateNativeOwnership, SessionMigrateNativeSource,
    };
    use crate::adapters::{DiscoveryRequest, SessionSource};
    use crate::domain::SessionKind;

    const HERMES_ID: &str = "20260830_123456_abcdef";
    const MASTRA_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const DEVIN_ID: &str = "repair-parser";

    #[test]
    #[ignore = "requires Session Migrate's sanitized native-client corpus"]
    fn reads_actual_native_client_databases() {
        let root = PathBuf::from(
            std::env::var_os("OAV_NATIVE_CORPUS_ROOT").expect("OAV_NATIVE_CORPUS_ROOT"),
        );
        for (provider, path) in [
            (
                Provider::Hermes,
                "hermes/0.20.6/portable-rich/native/state.db",
            ),
            (
                Provider::MastraCode,
                "mastracode/0.37.1/portable-rich/native/mastra.db",
            ),
            (
                Provider::Devin,
                "devin/3000.6.7/portable-rich/native/sessions.db",
            ),
        ] {
            let sessions = list(&provider, &root.join(path), None, 100).unwrap();
            assert!(!sessions.is_empty(), "{provider}");
            for session in sessions {
                assert!(!session.summary.is_empty(), "{provider} preview is empty");
                assert!(
                    session.updated_at.is_some(),
                    "{provider} activity is missing"
                );
                let exact =
                    list(&provider, &root.join(path), Some(&session.session_id), 1).unwrap();
                assert_eq!(exact.len(), 1);
                assert_eq!(exact[0].summary, session.summary);
            }
        }
    }

    fn fixture(provider: &Provider, path: &Path) -> Connection {
        let db = Connection::open(path).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        match provider {
            Provider::Hermes => db.execute_batch(&format!(
                "CREATE TABLE sessions(id TEXT PRIMARY KEY,cwd TEXT,title TEXT,model TEXT,started_at REAL,ended_at REAL,last_activity_at REAL,hidden INTEGER,archived INTEGER);
                 CREATE TABLE messages(id INTEGER PRIMARY KEY,session_id TEXT,role TEXT,content TEXT,timestamp REAL,active INTEGER);
                 CREATE INDEX messages_session ON messages(session_id,id);
                 INSERT INTO sessions VALUES('{HERMES_ID}','/work/demo','Hermes title','provider/model',1000,NULL,1001,0,0);
                 INSERT INTO messages VALUES(1,'{HERMES_ID}','user','hello',1001,1);
                 INSERT INTO messages VALUES(2,'{HERMES_ID}','assistant','latest Hermes reply',1002,1);
                 INSERT INTO messages VALUES(3,'{HERMES_ID}','assistant','rewound text',1003,0);"
            )).unwrap(),
            Provider::MastraCode => db.execute_batch(&format!(
                "CREATE TABLE mastra_threads(id TEXT PRIMARY KEY,resourceId TEXT,title TEXT,metadata BLOB,createdAt TEXT,updatedAt TEXT);
                 CREATE TABLE mastra_messages(id TEXT PRIMARY KEY,thread_id TEXT,content TEXT,role TEXT,createdAt TEXT);
                 CREATE INDEX messages_thread ON mastra_messages(thread_id,createdAt);
                 INSERT INTO mastra_threads VALUES('{MASTRA_ID}','work-demo','Mastra title',jsonb('{{\"projectPath\":\"/work/demo\",\"currentModelId\":\"provider/model\"}}'),'2026-08-30T12:00:00Z','2026-08-30T12:00:01Z');
                 INSERT INTO mastra_messages VALUES('m1','{MASTRA_ID}','{{\"format\":2,\"parts\":[{{\"type\":\"text\",\"text\":\"hello\"}}]}}','signal','2026-08-30T12:00:01Z');
                 INSERT INTO mastra_messages VALUES('m2','{MASTRA_ID}','{{\"format\":2,\"parts\":[{{\"type\":\"reasoning\",\"text\":\"private\"}},{{\"type\":\"text\",\"text\":\"latest Mastra reply\"}}]}}','assistant','2026-08-30T12:00:05Z');"
            )).unwrap(),
            Provider::Devin => db.execute_batch(&format!(
                "CREATE TABLE sessions(id TEXT PRIMARY KEY,working_directory TEXT,title TEXT,model TEXT,created_at INTEGER,last_activity_at INTEGER,hidden INTEGER,main_chain_id TEXT);
                 CREATE TABLE message_nodes(row_id INTEGER PRIMARY KEY,session_id TEXT,node_id TEXT,parent_node_id TEXT,chat_message TEXT);
                 CREATE INDEX nodes_session ON message_nodes(session_id,node_id);
                 INSERT INTO sessions VALUES('{DEVIN_ID}','/work/demo','Devin title','model',1000,1005,0,'tool');
                 INSERT INTO message_nodes VALUES(1,'{DEVIN_ID}','root',NULL,'{{\"role\":\"user\",\"content\":\"hello\"}}');
                 INSERT INTO message_nodes VALUES(2,'{DEVIN_ID}','reply','root','{{\"role\":\"assistant\",\"content\":\"latest Devin reply\"}}');
                 INSERT INTO message_nodes VALUES(3,'{DEVIN_ID}','tool','reply','{{\"role\":\"tool\",\"content\":\"tool output\"}}');
                 INSERT INTO message_nodes VALUES(4,'{DEVIN_ID}','inactive','root','{{\"role\":\"assistant\",\"content\":\"wrong branch\"}}');"
            )).unwrap(),
            _ => unreachable!(),
        }
        db
    }

    #[test]
    fn three_native_shapes_include_wal_latest_text_and_activity_not_initial_hello() {
        for (provider, id, text) in [
            (Provider::Hermes, HERMES_ID, "latest Hermes reply"),
            (Provider::MastraCode, MASTRA_ID, "latest Mastra reply"),
            (Provider::Devin, DEVIN_ID, "latest Devin reply"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("state.db");
            let db = fixture(&provider, &path); // keep WAL writer open
            let found = list(&provider, &path, Some(id), 1).unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].summary, text);
            assert!(found[0].updated_at > found[0].created_at);
            assert_eq!(found[0].cwd, PathBuf::from("/work/demo"));
            assert_eq!(list(&provider, &path, None, 100).unwrap().len(), 1);
            assert!(list(&provider, &path, None, 0).unwrap().is_empty());
            drop(db);
        }
    }

    #[test]
    fn owned_only_discovery_persists_after_restart_and_refreshes() {
        for (provider, id) in [
            (Provider::Hermes, HERMES_ID),
            (Provider::MastraCode, MASTRA_ID),
            (Provider::Devin, DEVIN_ID),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("state.db");
            let db = fixture(&provider, &path);
            let owner_path = dir.path().join("owner/owned.json");
            let owner =
                SessionMigrateNativeOwnership::load(provider.clone(), owner_path.clone()).unwrap();
            let source = SessionMigrateNativeSource::host(
                provider.clone(),
                "unused",
                path.clone(),
                owner.clone(),
            )
            .unwrap();
            let request = DiscoveryRequest::default();
            assert!(source.discover(&request).unwrap().is_empty());
            owner
                .inner
                .record(
                    id,
                    Path::new("/work/demo"),
                    "hello",
                    Some(&path),
                    provider.label(),
                )
                .unwrap();
            drop(source);
            drop(owner);
            let owner = SessionMigrateNativeOwnership::load(provider.clone(), owner_path).unwrap();
            let source =
                SessionMigrateNativeSource::host(provider.clone(), "unused", path, owner).unwrap();
            let first = source.discover(&request).unwrap();
            assert_eq!(first.len(), 1);
            assert_eq!(first[0].kind, SessionKind::Managed);
            match provider {
                Provider::Hermes => {
                    db.execute(
                        "UPDATE messages SET content='next reply',timestamp=2000 WHERE id=2",
                        [],
                    )
                    .unwrap();
                }
                Provider::MastraCode => {
                    db.execute("UPDATE mastra_messages SET content='{\"parts\":[{\"type\":\"text\",\"text\":\"next reply\"}]}',createdAt='2026-08-31T12:00:00Z' WHERE id='m2'",[]).unwrap();
                }
                Provider::Devin => {
                    db.execute("UPDATE message_nodes SET chat_message='{\"role\":\"assistant\",\"content\":\"next reply\"}' WHERE node_id='reply'",[]).unwrap();
                    db.execute("UPDATE sessions SET last_activity_at=2000", [])
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let next = source.discover(&request).unwrap();
            assert_eq!(next[0].summary, "next reply");
            assert!(next[0].updated_at > first[0].updated_at);
        }
    }

    #[test]
    fn missing_database_is_not_created_and_unknown_schema_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.db");
        assert!(list(&Provider::Hermes, &path, None, 1).unwrap().is_empty());
        assert!(!path.exists());
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE unrelated(id TEXT);")
            .unwrap();
        for provider in [Provider::Hermes, Provider::MastraCode, Provider::Devin] {
            assert!(list(&provider, &path, None, 1)
                .unwrap_err()
                .to_string()
                .contains("schema"));
        }
        assert!(db.prepare("SELECT * FROM unrelated").is_ok());
    }

    #[test]
    fn saved_models_are_deduplicated_and_jsonb_works_without_system_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        for provider in [Provider::Hermes, Provider::MastraCode] {
            let path = dir.path().join(format!("{}.db", provider.label()));
            let _db = fixture(&provider, &path);
            assert_eq!(saved_models(&provider, &path).unwrap(), ["provider/model"]);
        }
        assert_eq!(
            devin_models(r#"{"models":[{"id":"opus"},{"id":"opus"},"sonnet"]}"#).unwrap(),
            ["opus", "sonnet"]
        );
        assert!(devin_models("login required").is_err());
        assert!(devin_models("{}").is_err());
    }

    #[test]
    fn hidden_and_archived_rows_are_not_resurrected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hermes.db");
        let db = fixture(&Provider::Hermes, &path);
        for column in ["hidden", "archived"] {
            db.execute(&format!("UPDATE sessions SET {column}=1"), [])
                .unwrap();
            assert!(list(&Provider::Hermes, &path, Some(HERMES_ID), 1)
                .unwrap()
                .is_empty());
            db.execute(&format!("UPDATE sessions SET {column}=0"), [])
                .unwrap();
        }
    }

    #[test]
    fn malformed_ids_and_multiline_tasks_cannot_inject_cli_commands() {
        for provider in [Provider::Hermes, Provider::MastraCode, Provider::Devin] {
            for id in [
                "",
                "--help",
                "../outside",
                "id\r/quit",
                "id';DROP TABLE sessions;--",
            ] {
                assert!(validate_provider_id(&provider, id).is_err());
            }
        }
        assert_eq!(
            prompt_input("first\nsecond"),
            b"\x1b[200~first\nsecond\x1b[201~\r"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_sidecar_redirects_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let db = fixture(&Provider::Hermes, &path);
        let link = dir.path().join("link.db");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(list(&Provider::Hermes, &link, None, 1).is_err());
        drop(db);
        std::os::unix::fs::symlink(&path, dir.path().join("state.db-wal")).unwrap();
        assert!(list(&Provider::Hermes, &path, None, 1).is_err());
    }

    #[test]
    fn owned_lookup_is_bounded_with_ten_thousand_unrelated_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let db = fixture(&Provider::Hermes, &path);
        db.execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<10000)
            INSERT INTO sessions SELECT printf('20260830_123456_%06x',x),'/other','Unrelated','model',1000,NULL,1000,0,0 FROM n;").unwrap();
        let start = Instant::now();
        let found = list(&Provider::Hermes, &path, Some(HERMES_ID), 1).unwrap();
        assert_eq!(found.len(), 1);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
