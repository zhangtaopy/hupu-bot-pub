use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

use crate::posts::PostRow;
use crate::replies::ReplyRow;

pub fn open_db(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    create_tables(&conn)?;
    Ok(conn)
}

fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS replies (
            pid            INTEGER PRIMARY KEY,
            tid            INTEGER NOT NULL,
            puid           INTEGER,
            euid           TEXT,
            username       TEXT NOT NULL,
            content        TEXT NOT NULL,
            quote          INTEGER NOT NULL DEFAULT 0,
            quote_pid      INTEGER,
            quote_tid      INTEGER,
            quote_puid     INTEGER,
            quote_euid     TEXT,
            quote_username TEXT,
            quote_content  TEXT,
            quote_create_time INTEGER,
            create_time    INTEGER NOT NULL,
            light_count    INTEGER NOT NULL DEFAULT 0,
            unlight_count  INTEGER NOT NULL DEFAULT 0,
            title          TEXT NOT NULL,
            topic_id       INTEGER,
            topic_name     TEXT,
            format_time    TEXT,
            fetched_at     INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_replies_create_time ON replies(create_time);
        CREATE INDEX IF NOT EXISTS idx_replies_tid ON replies(tid);
        CREATE INDEX IF NOT EXISTS idx_replies_euid ON replies(euid);

        CREATE TABLE IF NOT EXISTS ai_analysis (
            euid         TEXT PRIMARY KEY,
            result       TEXT NOT NULL,
            created_at   INTEGER NOT NULL,
            updated_at   INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS similarity_analysis (
            euid         TEXT NOT NULL,
            threshold    REAL NOT NULL,
            result       TEXT NOT NULL,
            created_at   INTEGER NOT NULL,
            updated_at   INTEGER NOT NULL,
            PRIMARY KEY (euid, threshold)
        );

        CREATE TABLE IF NOT EXISTS posts (
            tid            INTEGER PRIMARY KEY,
            euid           TEXT NOT NULL,
            username       TEXT NOT NULL,
            title          TEXT NOT NULL,
            summary        TEXT NOT NULL DEFAULT '',
            create_time    INTEGER NOT NULL,
            lastpost_time  INTEGER NOT NULL DEFAULT 0,
            replies        INTEGER NOT NULL DEFAULT 0,
            visits         INTEGER NOT NULL DEFAULT 0,
            lights         INTEGER NOT NULL DEFAULT 0,
            recommend_num  INTEGER NOT NULL DEFAULT 0,
            forum_name     TEXT NOT NULL DEFAULT '',
            topic_name     TEXT NOT NULL DEFAULT '',
            topic_id       INTEGER NOT NULL DEFAULT 0,
            total_pics     INTEGER NOT NULL DEFAULT 0,
            has_video      INTEGER NOT NULL DEFAULT 0,
            share_num      INTEGER NOT NULL DEFAULT 0,
            format_time    TEXT,
            fetched_at     INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_posts_euid ON posts(euid);
        CREATE INDEX IF NOT EXISTS idx_posts_create_time ON posts(create_time);

        CREATE TABLE IF NOT EXISTS ai_post_analysis (
            euid         TEXT PRIMARY KEY,
            result       TEXT NOT NULL,
            created_at   INTEGER NOT NULL,
            updated_at   INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS ai_batch_errors (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            euid         TEXT NOT NULL,
            batch_type   TEXT NOT NULL,
            batch_index  INTEGER NOT NULL,
            error        TEXT NOT NULL,
            raw_response TEXT,
            created_at   INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_ai_batch_errors_euid ON ai_batch_errors(euid);",
    )?;
    Ok(())
}

pub fn upsert_replies(conn: &Connection, replies: &[ReplyRow]) -> Result<usize> {
    let now = chrono::Utc::now().timestamp();
    let tx = conn.unchecked_transaction()?;

    for r in replies {
        tx.execute(
            "INSERT OR REPLACE INTO replies (
                pid, tid, puid, euid, username, content,
                quote, quote_pid, quote_tid, quote_puid, quote_euid,
                quote_username, quote_content, quote_create_time,
                create_time, light_count, unlight_count,
                title, topic_id, topic_name, format_time, fetched_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10, ?11,
                ?12, ?13, ?14,
                ?15, ?16, ?17,
                ?18, ?19, ?20, ?21, ?22
            )",
            rusqlite::params![
                r.pid,
                r.tid,
                r.puid,
                r.euid,
                r.username,
                r.content,
                r.quote,
                r.quote_pid,
                r.quote_tid,
                r.quote_puid,
                r.quote_euid,
                r.quote_username,
                r.quote_content,
                r.quote_create_time,
                r.create_time,
                r.light_count,
                r.unlight_count,
                r.title,
                r.topic_id,
                r.topic_name,
                r.format_time,
                now,
            ],
        )?;
    }

    tx.commit()?;
    Ok(replies.len())
}

pub fn query_replies(
    conn: &Connection,
    euid: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<Vec<ReplyRow>> {
    let sql = match euid {
        Some(_) => "SELECT pid, tid, puid, euid, username, content,
                quote, quote_pid, quote_tid, quote_puid, quote_euid,
                quote_username, quote_content, quote_create_time,
                create_time, light_count, unlight_count,
                title, topic_id, topic_name, format_time
         FROM replies WHERE euid = ? ORDER BY create_time DESC LIMIT ? OFFSET ?",
        None => "SELECT pid, tid, puid, euid, username, content,
                quote, quote_pid, quote_tid, quote_puid, quote_euid,
                quote_username, quote_content, quote_create_time,
                create_time, light_count, unlight_count,
                title, topic_id, topic_name, format_time
         FROM replies ORDER BY create_time DESC LIMIT ? OFFSET ?",
    };

    let mut stmt = conn.prepare(sql)?;
    let rows = match euid {
        Some(uid) => stmt.query_map(rusqlite::params![uid, limit, offset], row_to_reply)?,
        None => stmt.query_map(rusqlite::params![limit, offset], row_to_reply)?,
    };

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

fn row_to_reply(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReplyRow> {
    Ok(ReplyRow {
        pid: row.get(0)?,
        tid: row.get(1)?,
        puid: row.get(2)?,
        euid: row.get(3)?,
        username: row.get(4)?,
        content: row.get(5)?,
        quote: row.get(6)?,
        quote_pid: row.get(7)?,
        quote_tid: row.get(8)?,
        quote_puid: row.get(9)?,
        quote_euid: row.get(10)?,
        quote_username: row.get(11)?,
        quote_content: row.get(12)?,
        quote_create_time: row.get(13)?,
        create_time: row.get(14)?,
        light_count: row.get(15)?,
        unlight_count: row.get(16)?,
        title: row.get(17)?,
        topic_id: row.get(18)?,
        topic_name: row.get(19)?,
        format_time: row.get(20)?,
    })
}

pub fn count_replies(conn: &Connection, euid: Option<&str>) -> Result<i64> {
    let sql = if euid.is_some() {
        "SELECT COUNT(*) FROM replies WHERE euid = ?1"
    } else {
        "SELECT COUNT(*) FROM replies"
    };

    let count: i64 = if let Some(uid) = euid {
        conn.query_row(sql, rusqlite::params![uid], |row| row.get(0))?
    } else {
        conn.query_row(sql, rusqlite::params![], |row| row.get(0))?
    };
    Ok(count)
}

// ── Posts ──

pub fn upsert_posts(conn: &Connection, posts: &[PostRow]) -> Result<usize> {
    let now = chrono::Utc::now().timestamp();
    let tx = conn.unchecked_transaction()?;

    for p in posts {
        tx.execute(
            "INSERT OR REPLACE INTO posts (
                tid, euid, username, title, summary,
                create_time, lastpost_time, replies, visits, lights,
                recommend_num, forum_name, topic_name, topic_id,
                total_pics, has_video, share_num, format_time, fetched_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, ?8, ?9, ?10,
                ?11, ?12, ?13, ?14,
                ?15, ?16, ?17, ?18, ?19
            )",
            rusqlite::params![
                p.tid, p.euid, p.username, p.title, p.summary,
                p.create_time, p.lastpost_time, p.replies, p.visits, p.lights,
                p.recommend_num, p.forum_name, p.topic_name, p.topic_id,
                p.total_pics, p.has_video as i64, p.share_num, p.format_time, now
            ],
        )?;
    }

    tx.commit()?;
    Ok(posts.len())
}

pub fn query_posts(
    conn: &Connection,
    euid: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<Vec<PostRow>> {
    let sql = match euid {
        Some(_) => "SELECT tid, euid, username, title, summary,
                create_time, lastpost_time, replies, visits, lights,
                recommend_num, forum_name, topic_name, topic_id,
                total_pics, has_video, share_num, format_time
         FROM posts WHERE euid = ? ORDER BY create_time DESC LIMIT ? OFFSET ?",
        None => "SELECT tid, euid, username, title, summary,
                create_time, lastpost_time, replies, visits, lights,
                recommend_num, forum_name, topic_name, topic_id,
                total_pics, has_video, share_num, format_time
         FROM posts ORDER BY create_time DESC LIMIT ? OFFSET ?",
    };

    let mut stmt = conn.prepare(sql)?;
    let rows = match euid {
        Some(uid) => stmt.query_map(rusqlite::params![uid, limit, offset], row_to_post)?,
        None => stmt.query_map(rusqlite::params![limit, offset], row_to_post)?,
    };

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

fn row_to_post(row: &rusqlite::Row<'_>) -> rusqlite::Result<PostRow> {
    Ok(PostRow {
        tid: row.get(0)?,
        euid: row.get(1)?,
        username: row.get(2)?,
        title: row.get(3)?,
        summary: row.get(4)?,
        create_time: row.get(5)?,
        lastpost_time: row.get(6)?,
        replies: row.get(7)?,
        visits: row.get(8)?,
        lights: row.get(9)?,
        recommend_num: row.get(10)?,
        forum_name: row.get(11)?,
        topic_name: row.get(12)?,
        topic_id: row.get(13)?,
        total_pics: row.get(14)?,
        has_video: row.get::<_, i64>(15)? != 0,
        share_num: row.get(16)?,
        format_time: row.get(17)?,
    })
}

pub fn count_posts(conn: &Connection, euid: Option<&str>) -> Result<i64> {
    let sql = if euid.is_some() {
        "SELECT COUNT(*) FROM posts WHERE euid = ?1"
    } else {
        "SELECT COUNT(*) FROM posts"
    };

    let count: i64 = if let Some(uid) = euid {
        conn.query_row(sql, rusqlite::params![uid], |row| row.get(0))?
    } else {
        conn.query_row(sql, rusqlite::params![], |row| row.get(0))?
    };
    Ok(count)
}

pub fn save_ai_post_analysis(conn: &Connection, euid: &str, result_json: &str) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT OR REPLACE INTO ai_post_analysis (euid, result, created_at, updated_at) VALUES (?1, ?2, COALESCE((SELECT created_at FROM ai_post_analysis WHERE euid = ?1), ?3), ?3)",
        rusqlite::params![euid, result_json, now],
    )?;
    Ok(())
}

pub fn get_ai_post_analysis(conn: &Connection, euid: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT result FROM ai_post_analysis WHERE euid = ?1")?;
    let mut rows = stmt.query_map(rusqlite::params![euid], |row| row.get::<_, String>(0))?;
    match rows.next() {
        Some(Ok(result)) => Ok(Some(result)),
        _ => Ok(None),
    }
}

pub fn save_ai_analysis(conn: &Connection, euid: &str, result_json: &str) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT OR REPLACE INTO ai_analysis (euid, result, created_at, updated_at) VALUES (?1, ?2, COALESCE((SELECT created_at FROM ai_analysis WHERE euid = ?1), ?3), ?3)",
        rusqlite::params![euid, result_json, now],
    )?;
    Ok(())
}

pub fn get_ai_analysis(conn: &Connection, euid: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT result FROM ai_analysis WHERE euid = ?1")?;
    let mut rows = stmt.query_map(rusqlite::params![euid], |row| row.get::<_, String>(0))?;
    match rows.next() {
        Some(Ok(result)) => Ok(Some(result)),
        _ => Ok(None),
    }
}

pub fn save_similarity_analysis(conn: &Connection, euid: &str, threshold: f64, result_json: &str) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT OR REPLACE INTO similarity_analysis (euid, threshold, result, created_at, updated_at)
         VALUES (?1, ?2, ?3, COALESCE((SELECT created_at FROM similarity_analysis WHERE euid = ?1 AND threshold = ?2), ?4), ?4)",
        rusqlite::params![euid, threshold, result_json, now],
    )?;
    Ok(())
}

pub fn get_similarity_analysis(conn: &Connection, euid: &str, threshold: f64) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT result FROM similarity_analysis WHERE euid = ?1 AND threshold = ?2")?;
    let mut rows = stmt.query_map(rusqlite::params![euid, threshold], |row| row.get::<_, String>(0))?;
    match rows.next() {
        Some(Ok(result)) => Ok(Some(result)),
        _ => Ok(None),
    }
}

pub fn get_all_euids(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let mut stmt = conn.prepare(
        "SELECT euid, username FROM replies WHERE euid IS NOT NULL AND euid != '' GROUP BY euid ORDER BY username"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (euid, username) = row?;
        map.entry(euid).or_insert(username);
    }

    let mut stmt2 = conn.prepare(
        "SELECT euid, username FROM posts WHERE euid IS NOT NULL AND euid != '' GROUP BY euid ORDER BY username"
    )?;
    let rows2 = stmt2.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows2 {
        let (euid, username) = row?;
        map.entry(euid).or_insert(username);
    }

    let mut result: Vec<(String, String)> = map.into_iter().collect();
    result.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(result)
}

pub fn save_batch_error(
    conn: &Connection,
    euid: &str,
    batch_type: &str,
    batch_index: usize,
    error: &str,
    raw_response: Option<&str>,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO ai_batch_errors (euid, batch_type, batch_index, error, raw_response, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![euid, batch_type, batch_index as i64, error, raw_response, now],
    )?;
    Ok(())
}