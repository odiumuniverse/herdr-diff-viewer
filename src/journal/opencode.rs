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

fn resolve_db(db: &Path, pane_cwd: &str, session_id: &str) -> Option<SessionRef> {
    let conn = connect(db)?;
    let (id, dir) = if !session_id.is_empty() {
        let dir: Option<String> = conn
            .query_row(
                "select directory from session where id = ?1",
                [session_id],
                |r| r.get(0),
            )
            .ok();
        (session_id.to_string(), dir)
    } else {
        let row: Option<(String, String)> = conn
            .query_row(
                "select id, directory from session where directory = ?1 order by time_updated desc limit 1",
                [pane_cwd],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        let (id, dir) = row?;
        (id, Some(dir))
    };
    Some(SessionRef {
        id,
        cwd: dir.or_else(|| Some(pane_cwd.to_string())),
        sources: vec![db.to_path_buf()],
        gated: false,
    })
}

fn edits_db(db: &Path, sess: &SessionRef, cursor: Option<&str>) -> Result<Edits, String> {
    let cur = cursor
        .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let cur_ts = cur.get("ts").and_then(|v| v.as_i64()).unwrap_or(0);
    let cur_ids: Vec<String> = cur
        .get("ids")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let conn = connect(db).ok_or_else(|| "open db".to_string())?;
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
        .query_map(rusqlite::params![sess.id, cur_ts], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut paths = Vec::new();
    let mut last_ts = cur_ts;
    let mut last_ids = cur_ids;
    for row in rows {
        let Ok((id, ts, fp, patch)) = row else {
            continue;
        };
        if ts < cur_ts || (ts == cur_ts && last_ids.contains(&id)) {
            continue;
        }
        if !fp.is_empty() {
            paths.push(PathBuf::from(fp));
        }
        if !patch.is_empty() {
            paths.extend(patch_paths(&patch));
        }
        if ts > last_ts {
            last_ts = ts;
            last_ids.clear();
        }
        if ts == last_ts {
            last_ids.push(id);
        }
    }
    let next = serde_json::json!({ "ts": last_ts, "ids": last_ids });
    Ok(Edits {
        paths: absolutize(paths, sess.cwd.as_deref()),
        cursor: next.to_string(),
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

    #[test]
    fn resolves_by_id_and_cwd_and_reads_edits() {
        let dir = std::env::temp_dir().join(format!("dv-oc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("opencode.db");
        setup_db(&db);
        let _guard = crate::state::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("OPENCODE_DB", &db);
        std::env::set_var("DIFF_JOURNAL_ROOT", &dir);
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

        std::env::remove_var("OPENCODE_DB");
        std::env::remove_var("DIFF_JOURNAL_ROOT");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
