use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::{absolutize, data_dir, patch_paths, Adapter, Edits, SessionRef};

pub struct Opencode;
pub struct Kilo;

fn db_path(kind: &str) -> PathBuf {
    if kind == "kilo" {
        if let Ok(x) = std::env::var("KILO_DB") {
            if !x.is_empty() {
                return PathBuf::from(x);
            }
        }
        data_dir("kilo").join("kilo.db")
    } else {
        if let Ok(x) = std::env::var("OPENCODE_DB") {
            if !x.is_empty() {
                return PathBuf::from(x);
            }
        }
        data_dir("opencode").join("opencode.db")
    }
}

fn connect(db: &Path) -> Option<Connection> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let _ = conn.busy_timeout(Duration::from_millis(300));
    Some(conn)
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "select 1 from sqlite_master where type = 'table' and name = ?1",
        [name],
        |_| Ok(()),
    )
    .is_ok()
}

/// opencode >= 2.0 moved sessions to `session_v2` and message parts into
/// `session_message.data.content[]`; older builds keep `session` + `part`.
/// Migrated sessions keep their history in `part` while new edits land in
/// `session_message`, so both are read and merged.
fn session_dir(conn: &Connection, id: &str) -> Option<String> {
    for table in ["session_v2", "session"] {
        if !table_exists(conn, table) {
            continue;
        }
        let sql = format!("select directory from {table} where id = ?1");
        if let Ok(dir) = conn.query_row(&sql, [id], |r| r.get::<_, String>(0)) {
            return Some(dir);
        }
    }
    None
}

fn newest_session(conn: &Connection, dir: &str) -> Option<(String, String)> {
    let mut best: Option<(i64, String, String)> = None;
    for table in ["session", "session_v2"] {
        if !table_exists(conn, table) {
            continue;
        }
        let sql = format!(
            "select id, directory, time_updated from {table} \
             where directory = ?1 order by time_updated desc limit 1"
        );
        let row = conn.query_row(&sql, [dir], |r| {
            Ok((
                r.get::<_, i64>(2)?,
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
            ))
        });
        if let Ok((ts, id, d)) = row {
            if best.as_ref().is_none_or(|(prev, _, _)| ts > *prev) {
                best = Some((ts, id, d));
            }
        }
    }
    best.map(|(_, id, dir)| (id, dir))
}

fn resolve_db(db: &Path, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
    let conn = connect(db)?;
    let (id, dir) = if !session_id.is_empty() {
        (session_id.to_string(), session_dir(&conn, session_id))
    } else {
        let (id, dir) = newest_session(&conn, pane_cwd)?;
        (id, Some(dir))
    };
    Some(SessionRef {
        id,
        cwd: dir.or_else(|| Some(pane_cwd.to_string())),
        sources: vec![db.to_path_buf()],
        gated: false,
    })
}

struct Cursor {
    ts: i64,
    ids: Vec<String>,
}

fn parse_cursor(cursor: Option<&str>) -> Cursor {
    let raw = cursor
        .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    Cursor {
        ts: raw.get("ts").and_then(|v| v.as_i64()).unwrap_or(0),
        ids: raw
            .get("ids")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn consumed(cur: &Cursor, ts: i64, id: &str) -> bool {
    ts < cur.ts || (ts == cur.ts && cur.ids.iter().any(|x| x == id))
}

fn advance(last_ts: &mut i64, last_ids: &mut Vec<String>, ts: i64, id: String) {
    if ts > *last_ts {
        *last_ts = ts;
        last_ids.clear();
    }
    if ts == *last_ts {
        last_ids.push(id);
    }
}

struct Row {
    id: String,
    ts: i64,
    paths: Vec<PathBuf>,
}

fn parts_v1(conn: &Connection, sess: &SessionRef, since: i64) -> Result<Vec<Row>, String> {
    if !table_exists(conn, "part") {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "select id, time_updated,
                    coalesce(json_extract(data,'$.state.input.filePath'),''),
                    coalesce(json_extract(data,'$.state.input.patchText'),'')
             from part
             where session_id = ?1 and time_updated >= ?2
               and json_extract(data,'$.tool') in ('edit','write','apply_patch')
             order by time_updated asc, id asc",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![sess.id, since], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        let Ok((id, ts, fp, patch)) = row else {
            continue;
        };
        let mut paths = Vec::new();
        if !fp.is_empty() {
            paths.push(PathBuf::from(fp));
        }
        if !patch.is_empty() {
            paths.extend(patch_paths(&patch));
        }
        out.push(Row { id, ts, paths });
    }
    Ok(out)
}

fn v2_paths(msg: &serde_json::Value) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let Some(content) = msg.get("content").and_then(|c| c.as_array()) else {
        return paths;
    };
    for part in content {
        if part.get("type").and_then(|t| t.as_str()) != Some("tool") {
            continue;
        }
        let name = part.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let input = part.pointer("/state/input");
        match name {
            "edit" | "write" => {
                let p = input
                    .and_then(|i| i.get("filePath").or_else(|| i.get("path")))
                    .and_then(|p| p.as_str());
                if let Some(p) = p {
                    paths.push(PathBuf::from(p));
                }
            }
            "apply_patch" | "patch" => {
                if let Some(text) = input
                    .and_then(|i| i.get("patchText"))
                    .and_then(|p| p.as_str())
                {
                    paths.extend(patch_paths(text));
                }
            }
            _ => {}
        }
    }
    paths
}

fn messages_v2(conn: &Connection, sess: &SessionRef, since: i64) -> Result<Vec<Row>, String> {
    if !table_exists(conn, "session_v2") || !table_exists(conn, "session_message") {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "with recursive tree(id) as (
                 select ?1
                 union all
                 select s.id from session_v2 s join tree t on s.parent_id = t.id
             )
             select m.id, m.time_updated, m.data
             from session_message m
             where m.session_id in (select id from tree)
               and m.type = 'assistant' and m.time_updated >= ?2
             order by m.time_updated asc, m.id asc",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![sess.id, since], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        let Ok((id, ts, data)) = row else {
            continue;
        };
        let paths = serde_json::from_str::<serde_json::Value>(&data)
            .map(|v| v2_paths(&v))
            .unwrap_or_default();
        out.push(Row { id, ts, paths });
    }
    Ok(out)
}

fn edits_db(db: &Path, sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
    let cur = parse_cursor(cursor);
    let conn = connect(db).ok_or_else(|| "open db".to_string())?;
    let mut rows = parts_v1(&conn, sess, cur.ts)?;
    rows.extend(messages_v2(&conn, sess, cur.ts)?);
    rows.sort_by(|a, b| (a.ts, &a.id).cmp(&(b.ts, &b.id)));

    let mut paths = Vec::new();
    let mut last_ts = cur.ts;
    let mut last_ids = cur.ids.clone();
    for row in rows {
        if consumed(&cur, row.ts, &row.id) {
            continue;
        }
        paths.extend(row.paths);
        advance(&mut last_ts, &mut last_ids, row.ts, row.id);
    }
    Ok(Edits {
        paths: absolutize(paths, sess.cwd.as_deref()),
        cursor: serde_json::json!({ "ts": last_ts, "ids": last_ids }).to_string(),
    })
}

impl Adapter for Opencode {
    fn resolve(&self, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
        resolve_db(&db_path("opencode"), pane_cwd, session_id)
    }

    fn edits(&self, sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
        edits_db(&db_path("opencode"), sess, cursor)
    }
}

impl Adapter for Kilo {
    fn resolve(&self, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
        resolve_db(&db_path("kilo"), pane_cwd, session_id)
    }

    fn edits(&self, sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
        edits_db(&db_path("kilo"), sess, cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "create table session (id text primary key, directory text not null, time_updated integer not null);
             create table part (id text primary key, message_id text not null, session_id text not null, time_created integer not null, time_updated integer not null, data text not null);",
        )
        .unwrap();
        conn.execute(
            "insert into session values ('ses_test', '/tmp/proj', 100)",
            [],
        )
        .unwrap();
        let data = serde_json::json!({
            "type": "tool",
            "tool": "edit",
            "state": {"input": {"filePath": "/tmp/proj/src/a.rs"}}
        });
        conn.execute(
            "insert into part values ('p1', 'm1', 'ses_test', 1, 100, ?1)",
            [data.to_string()],
        )
        .unwrap();
        let patch = serde_json::json!({
            "type": "tool",
            "tool": "apply_patch",
            "state": {"input": {"patchText": "*** Update File: src/b.rs\n*** Add File: src/c.rs\n"}}
        });
        conn.execute(
            "insert into part values ('p2', 'm1', 'ses_test', 1, 101, ?1)",
            [patch.to_string()],
        )
        .unwrap();
    }

    fn setup_db_v2(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "create table session_v2 (id text primary key, parent_id text, directory text not null, time_updated integer not null);
             create table session_message (id text primary key, session_id text not null, type text not null, seq integer not null, time_created integer not null, time_updated integer not null, data text not null);",
        )
        .unwrap();
        conn.execute(
            "insert into session_v2 values ('ses_parent', null, '/tmp/proj', 100)",
            [],
        )
        .unwrap();
        conn.execute(
            "insert into session_v2 values ('ses_child', 'ses_parent', '/tmp/proj', 101)",
            [],
        )
        .unwrap();
        let msgs = [
            (
                "p1",
                "ses_parent",
                100,
                serde_json::json!({"content":[{"type":"tool","name":"edit","state":{"input":{"filePath":"/tmp/proj/src/a.rs"}}}]}),
            ),
            (
                "p2",
                "ses_parent",
                101,
                serde_json::json!({"content":[{"type":"tool","name":"edit","state":{"input":{"path":"/tmp/proj/src/b.rs"}}},{"type":"text","text":"hi"}]}),
            ),
            (
                "p3",
                "ses_parent",
                102,
                serde_json::json!({"content":[{"type":"tool","name":"read","state":{"input":{"path":"/tmp/proj/src/skip.rs"}}}]}),
            ),
            (
                "p4",
                "ses_parent",
                103,
                serde_json::json!({"content":[{"type":"tool","name":"write","state":{"input":{"path":"/tmp/proj/src/c.rs"}}}]}),
            ),
            (
                "p5",
                "ses_parent",
                104,
                serde_json::json!({"content":[{"type":"tool","name":"apply_patch","state":{"input":{"patchText":"*** Update File: src/d.rs\n*** Add File: src/e.rs\n"}}}]}),
            ),
            (
                "p6",
                "ses_child",
                105,
                serde_json::json!({"content":[{"type":"tool","name":"edit","state":{"input":{"path":"/tmp/proj/src/f.rs"}}}]}),
            ),
        ];
        for (id, session, ts, data) in msgs {
            conn.execute(
                "insert into session_message values (?1, ?2, 'assistant', 1, ?3, ?3, ?4)",
                rusqlite::params![id, session, ts, data.to_string()],
            )
            .unwrap();
        }
    }

    /// Migrated database: same session id present in `session` and
    /// `session_v2`, old edits in `part`, new ones in `session_message`.
    fn setup_db_migrated(path: &Path) {
        setup_db(path);
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "create table session_v2 (id text primary key, parent_id text, directory text not null, time_updated integer not null);
             create table session_message (id text primary key, session_id text not null, type text not null, seq integer not null, time_created integer not null, time_updated integer not null, data text not null);",
        )
        .unwrap();
        conn.execute(
            "insert into session_v2 values ('ses_test', null, '/tmp/proj', 200)",
            [],
        )
        .unwrap();
        let data = serde_json::json!({
            "content": [{"type": "tool", "name": "write", "state": {"input": {"path": "/tmp/proj/src/new.rs"}}}]
        });
        conn.execute(
            "insert into session_message values ('msg_1', 'ses_test', 'assistant', 1, 200, 200, ?1)",
            [data.to_string()],
        )
        .unwrap();
    }

    fn with_db(label: &str, f: impl FnOnce(&Path)) {
        let dir = std::env::temp_dir().join(format!("dv-oc-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _guard = crate::state::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("DIFF_JOURNAL_ROOT", &dir);
        f(&dir);
        std::env::remove_var("OPENCODE_DB");
        std::env::remove_var("DIFF_JOURNAL_ROOT");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolves_by_id_and_cwd_and_reads_edits() {
        with_db("v1", |dir| {
            let db = dir.join("opencode.db");
            setup_db(&db);
            std::env::set_var("OPENCODE_DB", &db);
            let adapter = Opencode;

            let sess = adapter.resolve("/tmp/proj", "ses_test").expect("by id");
            assert_eq!(sess.cwd.as_deref(), Some("/tmp/proj"));
            let edits = adapter.edits(&sess, None).expect("edits");
            assert_eq!(
                edits.paths,
                vec![
                    PathBuf::from("/tmp/proj/src/a.rs"),
                    PathBuf::from("/tmp/proj/src/b.rs"),
                    PathBuf::from("/tmp/proj/src/c.rs"),
                ]
            );
            let again = adapter
                .edits(&sess, Some(&edits.cursor))
                .expect("idempotent");
            assert!(again.paths.is_empty());

            let by_cwd = adapter.resolve("/tmp/proj", "").expect("by cwd");
            assert_eq!(by_cwd.id, "ses_test");
        });
    }

    #[test]
    fn reads_v2_sessions_including_subagents() {
        with_db("v2", |dir| {
            let db = dir.join("opencode.db");
            setup_db_v2(&db);
            std::env::set_var("OPENCODE_DB", &db);
            let adapter = Opencode;

            let sess = adapter.resolve("/tmp/proj", "ses_parent").expect("by id");
            assert_eq!(sess.cwd.as_deref(), Some("/tmp/proj"));
            let edits = adapter.edits(&sess, None).expect("edits");
            assert_eq!(
                edits.paths,
                vec![
                    PathBuf::from("/tmp/proj/src/a.rs"),
                    PathBuf::from("/tmp/proj/src/b.rs"),
                    PathBuf::from("/tmp/proj/src/c.rs"),
                    PathBuf::from("/tmp/proj/src/d.rs"),
                    PathBuf::from("/tmp/proj/src/e.rs"),
                    PathBuf::from("/tmp/proj/src/f.rs"),
                ]
            );
            let again = adapter
                .edits(&sess, Some(&edits.cursor))
                .expect("idempotent");
            assert!(again.paths.is_empty());

            let child = adapter.resolve("/tmp/proj", "ses_child").expect("child id");
            let child_edits = adapter.edits(&child, None).expect("child edits");
            assert_eq!(child_edits.paths, vec![PathBuf::from("/tmp/proj/src/f.rs")]);

            let by_cwd = adapter.resolve("/tmp/proj", "").expect("by cwd");
            assert_eq!(by_cwd.id, "ses_child", "newest row wins");

            let unknown = adapter
                .resolve("/tmp/proj", "ses_missing")
                .expect("unknown");
            assert_eq!(unknown.cwd.as_deref(), Some("/tmp/proj"));
            assert!(adapter.edits(&unknown, None).unwrap().paths.is_empty());
        });
    }

    #[test]
    fn merges_migrated_history_from_both_schemas() {
        with_db("migrated", |dir| {
            let db = dir.join("opencode.db");
            setup_db_migrated(&db);
            std::env::set_var("OPENCODE_DB", &db);
            let adapter = Opencode;

            let sess = adapter.resolve("/tmp/proj", "ses_test").expect("by id");
            let edits = adapter.edits(&sess, None).expect("edits");
            assert_eq!(
                edits.paths,
                vec![
                    PathBuf::from("/tmp/proj/src/a.rs"),
                    PathBuf::from("/tmp/proj/src/b.rs"),
                    PathBuf::from("/tmp/proj/src/c.rs"),
                    PathBuf::from("/tmp/proj/src/new.rs"),
                ]
            );
            let again = adapter
                .edits(&sess, Some(&edits.cursor))
                .expect("idempotent");
            assert!(again.paths.is_empty());
        });
    }
}
