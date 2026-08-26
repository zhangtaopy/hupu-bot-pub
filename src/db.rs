use anyhow::Result;
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap, HashSet};
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
    // Migration: add ai_raw_json column if missing (added later)
    let _ = conn.execute_batch(
        "ALTER TABLE monitor_snapshots ADD COLUMN ai_raw_json TEXT;"
    );

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

        CREATE INDEX IF NOT EXISTS idx_ai_batch_errors_euid ON ai_batch_errors(euid);

        -- 分区舆论监控：帖子
        CREATE TABLE IF NOT EXISTS monitor_posts (
            tid            INTEGER PRIMARY KEY,
            topic_id       TEXT NOT NULL,
            title          TEXT NOT NULL,
            author         TEXT NOT NULL DEFAULT '',
            reply_count    INTEGER NOT NULL DEFAULT 0,
            light_count    INTEGER NOT NULL DEFAULT 0,
            create_time    INTEGER NOT NULL,
            format_time    TEXT,
            fetched_at     INTEGER NOT NULL,
            fetch_date     TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_monitor_posts_topic ON monitor_posts(topic_id);
        CREATE INDEX IF NOT EXISTS idx_monitor_posts_fetch_date ON monitor_posts(fetch_date);

        -- 分区舆论监控：热评
        CREATE TABLE IF NOT EXISTS monitor_replies (
            pid            INTEGER PRIMARY KEY,
            tid            INTEGER NOT NULL,
            topic_id       TEXT NOT NULL,
            username       TEXT NOT NULL DEFAULT '',
            content        TEXT NOT NULL,
            light_count    INTEGER NOT NULL DEFAULT 0,
            create_time    INTEGER NOT NULL,
            format_time    TEXT,
            fetched_at     INTEGER NOT NULL,
            fetch_date     TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_monitor_replies_topic ON monitor_replies(topic_id);
        CREATE INDEX IF NOT EXISTS idx_monitor_replies_tid ON monitor_replies(tid);
        CREATE INDEX IF NOT EXISTS idx_monitor_replies_fetch_date ON monitor_replies(fetch_date);

        -- AI 玩法：成分卡缓存（查成分）
        CREATE TABLE IF NOT EXISTS ghost_profile (
            euid         TEXT PRIMARY KEY,
            result       TEXT NOT NULL,
            created_at   INTEGER NOT NULL,
            updated_at   INTEGER NOT NULL
        );

        -- 分区舆论监控：每日快照（AI分析结果缓存）
        CREATE TABLE IF NOT EXISTS monitor_snapshots (
            topic_id       TEXT NOT NULL,
            snapshot_date  TEXT NOT NULL,
            post_count     INTEGER NOT NULL DEFAULT 0,
            reply_count    INTEGER NOT NULL DEFAULT 0,
            sentiment_dist TEXT,
            top_keywords   TEXT,
            ai_summary     TEXT,
            ai_raw_json    TEXT,
            created_at     INTEGER NOT NULL,
            PRIMARY KEY (topic_id, snapshot_date)
        );",
    )?;

    // Migration: analyzed_until 游标列（增量 AI 分析，added later）。
    // 语义：该 euid 的缓存结果已覆盖到「fetched_at <= analyzed_until」的全部数据行。
    add_analyzed_until_columns(conn)?;
    // 对存量缓存回填游标：分析完成时刻（updated_at）之前已抓取的数据视为已覆盖，
    // 纯 SQL 完成、不重新调用 AI；有批量失败记录的 euid 不回填（下次自动全量重试）。
    backfill_analysis_cursors(conn)?;

    // Migration: ai_analyzed 行级标记列（增量 AI 分析 v2，added later）。
    // fetched_at 游标在「重复抓取会刷新所有行 fetched_at」时会把历史行误判为新数据，
    // 因此改为行级标记：已分析的行打标，增量只取 ai_analyzed = 0 的行。
    add_ai_analyzed_columns(conn)?;
    // 对存量数据回填行标记：有过成功分析（analyzed_until > 0）且行在分析完成时刻
    // （updated_at）之前已抓取 → 视为已分析。纯 SQL 完成、不重新调用 AI。
    backfill_ai_analyzed_flags(conn)?;

    Ok(())
}

/// Migration: 为 ai_analysis / ai_post_analysis 增加 analyzed_until 游标列（幂等）。
fn add_analyzed_until_columns(conn: &Connection) -> Result<()> {
    for table in ["ai_analysis", "ai_post_analysis"] {
        let sql = format!("PRAGMA table_info({table})");
        let has = conn
            .prepare(&sql)?
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|name| name == "analyzed_until");
        if !has {
            conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN analyzed_until INTEGER NOT NULL DEFAULT 0;"
            ))?;
        }
    }
    Ok(())
}

/// Migration: 为 replies / posts 增加 ai_analyzed 行级标记列（幂等）。
/// 1 = 该行内容已被 AI 分析覆盖；增量分析只处理 0 的行。
fn add_ai_analyzed_columns(conn: &Connection) -> Result<()> {
    for table in ["replies", "posts"] {
        let sql = format!("PRAGMA table_info({table})");
        let has = conn
            .prepare(&sql)?
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|name| name == "ai_analyzed");
        if !has {
            conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN ai_analyzed INTEGER NOT NULL DEFAULT 0;"
            ))?;
        }
    }
    Ok(())
}

/// Migration: 对存量数据回填 ai_analyzed 行标记（幂等，重复执行无副作用）。
///
/// 判据：该用户有 AI 分析缓存（= 成功全量分析的产物）且无批次失败记录，
/// 且行要么在分析完成时刻（updated_at）之前抓取，要么内容时间（create_time）
/// 不晚于分析完成时刻。两者取 OR 是为了覆盖「缓存生成后全量重抓」的场景：
/// 重抓会刷新所有行的 fetched_at，但 create_time 是内容发表时间、不会被刷新，
/// 因此仍能证明这些老内容在分析时已存在；而缓存后新发表的内容
/// （create_time > updated_at）保持未标记，由增量分析处理。
/// 有批次失败记录的用户不标记，下次增量分析自动对其全量重试。
fn backfill_ai_analyzed_flags(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "UPDATE replies SET ai_analyzed = 1
         WHERE EXISTS (
             SELECT 1 FROM ai_analysis a
             WHERE a.euid = replies.euid
               AND NOT EXISTS (
                   SELECT 1 FROM ai_batch_errors e
                   WHERE e.euid = a.euid AND e.batch_type = 'reply'
               )
               AND (replies.fetched_at <= a.updated_at
                    OR replies.create_time <= a.updated_at)
         );",
    )?;
    conn.execute_batch(
        "UPDATE posts SET ai_analyzed = 1
         WHERE EXISTS (
             SELECT 1 FROM ai_post_analysis a
             WHERE a.euid = posts.euid
               AND NOT EXISTS (
                   SELECT 1 FROM ai_batch_errors e
                   WHERE e.euid = a.euid AND e.batch_type = 'post'
               )
               AND (posts.fetched_at <= a.updated_at
                    OR posts.create_time <= a.updated_at)
         );",
    )?;
    Ok(())
}

// ── 增量 AI 分析：行级未分析标记 ──

/// 该用户尚未被 AI 分析的回复行数（增量分析的工作量）。
pub fn count_unanalyzed_replies(conn: &Connection, euid: &str) -> Result<i64> {
    let v: i64 = conn.query_row(
        "SELECT COUNT(*) FROM replies WHERE euid = ?1 AND ai_analyzed = 0",
        rusqlite::params![euid],
        |row| row.get(0),
    )?;
    Ok(v)
}

/// 该用户尚未被 AI 分析的发帖行数。
pub fn count_unanalyzed_posts(conn: &Connection, euid: &str) -> Result<i64> {
    let v: i64 = conn.query_row(
        "SELECT COUNT(*) FROM posts WHERE euid = ?1 AND ai_analyzed = 0",
        rusqlite::params![euid],
        |row| row.get(0),
    )?;
    Ok(v)
}

/// 取该用户尚未被 AI 分析的回帖（按 create_time 降序），供增量分析使用。
pub fn query_unanalyzed_replies(conn: &Connection, euid: &str) -> Result<Vec<ReplyRow>> {
    let mut stmt = conn.prepare(
        "SELECT pid, tid, puid, euid, username, content,
                quote, quote_pid, quote_tid, quote_puid, quote_euid,
                quote_username, quote_content, quote_create_time,
                create_time, light_count, unlight_count,
                title, topic_id, topic_name, format_time
         FROM replies WHERE euid = ? AND ai_analyzed = 0
         ORDER BY create_time DESC",
    )?;
    let rows = stmt.query_map(rusqlite::params![euid], row_to_reply)?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// 取该用户尚未被 AI 分析的发帖（按 create_time 降序），供增量分析使用。
pub fn query_unanalyzed_posts(conn: &Connection, euid: &str) -> Result<Vec<PostRow>> {
    let mut stmt = conn.prepare(
        "SELECT tid, euid, username, title, summary,
                create_time, lastpost_time, replies, visits, lights,
                recommend_num, forum_name, topic_name, topic_id,
                total_pics, has_video, share_num, format_time
         FROM posts WHERE euid = ? AND ai_analyzed = 0
         ORDER BY create_time DESC",
    )?;
    let rows = stmt.query_map(rusqlite::params![euid], row_to_post)?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// 将该用户全部未标记回帖标记为已分析（全量分析或增量分析完成后调用）。
pub fn mark_replies_analyzed(conn: &Connection, euid: &str) -> Result<usize> {
    let n = conn.execute(
        "UPDATE replies SET ai_analyzed = 1 WHERE euid = ?1 AND ai_analyzed = 0",
        rusqlite::params![euid],
    )?;
    Ok(n)
}

/// 将该用户全部未标记发帖标记为已分析。
pub fn mark_posts_analyzed(conn: &Connection, euid: &str) -> Result<usize> {
    let n = conn.execute(
        "UPDATE posts SET ai_analyzed = 1 WHERE euid = ?1 AND ai_analyzed = 0",
        rusqlite::params![euid],
    )?;
    Ok(n)
}

/// Migration: 对存量 AI 分析缓存回填 analyzed_until 游标（幂等，重复执行无副作用）。
///
/// 回填值 = 「缓存保存时刻（updated_at）之前已抓取」的数据行的最大 fetched_at。
/// 这样缓存生成之后新抓取的行（fetched_at > updated_at）不会被误标为已分析，
/// 增量逻辑会自动识别它们；旧数据零 AI 调用完成标记。
fn backfill_analysis_cursors(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "UPDATE ai_analysis
         SET analyzed_until = COALESCE((
             SELECT MAX(fetched_at) FROM replies
             WHERE replies.euid = ai_analysis.euid
               AND replies.fetched_at <= ai_analysis.updated_at
         ), 0)
         WHERE analyzed_until = 0
           AND NOT EXISTS (
               SELECT 1 FROM ai_batch_errors
               WHERE ai_batch_errors.euid = ai_analysis.euid
                 AND ai_batch_errors.batch_type = 'reply'
           );",
    )?;
    conn.execute_batch(
        "UPDATE ai_post_analysis
         SET analyzed_until = COALESCE((
             SELECT MAX(fetched_at) FROM posts
             WHERE posts.euid = ai_post_analysis.euid
               AND posts.fetched_at <= ai_post_analysis.updated_at
         ), 0)
         WHERE analyzed_until = 0
           AND NOT EXISTS (
               SELECT 1 FROM ai_batch_errors
               WHERE ai_batch_errors.euid = ai_post_analysis.euid
                 AND ai_batch_errors.batch_type = 'post'
           );",
    )?;
    Ok(())
}

pub fn upsert_replies(conn: &Connection, replies: &[ReplyRow]) -> Result<usize> {
    let now = chrono::Utc::now().timestamp();
    let tx = conn.unchecked_transaction()?;

    for r in replies {
        tx.execute(
            "INSERT INTO replies (
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
            )
            ON CONFLICT(pid) DO UPDATE SET
                tid = excluded.tid,
                puid = excluded.puid,
                euid = excluded.euid,
                username = excluded.username,
                content = excluded.content,
                quote = excluded.quote,
                quote_pid = excluded.quote_pid,
                quote_tid = excluded.quote_tid,
                quote_puid = excluded.quote_puid,
                quote_euid = excluded.quote_euid,
                quote_username = excluded.quote_username,
                quote_content = excluded.quote_content,
                quote_create_time = excluded.quote_create_time,
                create_time = excluded.create_time,
                light_count = excluded.light_count,
                unlight_count = excluded.unlight_count,
                title = excluded.title,
                topic_id = excluded.topic_id,
                topic_name = excluded.topic_name,
                format_time = excluded.format_time,
                fetched_at = excluded.fetched_at
                -- 注意: ai_analyzed 不在更新列表 → 重抓保留已分析标记",
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

/// 批量查询在一组 pid 中已存在于 replies 表的 pid 集合（用于增量爬取判重）。
pub fn existing_reply_pids(conn: &Connection, pids: &[i64]) -> Result<HashSet<i64>> {
    if pids.is_empty() {
        return Ok(HashSet::new());
    }
    let placeholders = vec!["?"; pids.len()].join(",");
    let sql = format!("SELECT pid FROM replies WHERE pid IN ({})", placeholders);
    let mut stmt = conn.prepare(&sql)?;
    let params = rusqlite::params_from_iter(pids.iter().copied());
    let rows = stmt.query_map(params, |row| row.get::<_, i64>(0))?;
    let mut set = HashSet::new();
    for row in rows {
        set.insert(row?);
    }
    Ok(set)
}

/// 批量查询在一组 tid 中已存在于 posts 表的 tid 集合（用于增量爬取判重）。
pub fn existing_post_tids(conn: &Connection, tids: &[i64]) -> Result<HashSet<i64>> {
    if tids.is_empty() {
        return Ok(HashSet::new());
    }
    let placeholders = vec!["?"; tids.len()].join(",");
    let sql = format!("SELECT tid FROM posts WHERE tid IN ({})", placeholders);
    let mut stmt = conn.prepare(&sql)?;
    let params = rusqlite::params_from_iter(tids.iter().copied());
    let rows = stmt.query_map(params, |row| row.get::<_, i64>(0))?;
    let mut set = HashSet::new();
    for row in rows {
        set.insert(row?);
    }
    Ok(set)
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

// ── 互动图谱（社交图） ──
//
// 基于 replies 表的 quote_* 字段构建"谁引用了谁"的单向关系图：
// 中心节点 = 被分析用户，边 = 该用户引用某人的回复关系，权重 = 引用次数。
// 无需额外抓取，数据全部来自已入库的回帖记录。

#[derive(Debug, Clone, serde::Serialize)]
pub struct InteractionQuote {
    pub content: String,
    pub light_count: i64,
    pub format_time: String,
    pub title: String,
    pub quote_content: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InteractionNode {
    pub name: String,
    pub is_main: bool,
    pub count: i64,
    pub light_sum: i64,
    pub first_time: String,
    pub last_time: String,
    pub top_quotes: Vec<InteractionQuote>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InteractionEdge {
    pub source: String,
    pub target: String,
    pub count: i64,
    pub light_sum: i64,
    pub top_quotes: Vec<InteractionQuote>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InteractionGraph {
    pub main_username: String,
    pub total_interactions: i64,
    pub total_targets: i64,
    pub shown_targets: usize,
    pub nodes: Vec<InteractionNode>,
    pub edges: Vec<InteractionEdge>,
}

fn fmt_ts(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// 不是真实用户、而是系统统称的引用对象，图谱中过滤掉
const COLLECTIVE_NAMES: [&str; 1] = ["小黑屋住户"];

/// 聚合某用户的引用互动数据，返回图谱（节点 + 边）。
/// `max_nodes` 限制展示的互动对象数量（按互动次数降序截断）。
pub fn query_interaction_graph(
    conn: &Connection,
    euid: &str,
    max_nodes: usize,
) -> Result<InteractionGraph> {
    let main_username = get_username(conn, euid)?.unwrap_or_else(|| "未知用户".to_string());

    // 主节点统计：总回帖、总点亮、时间范围
    let (main_count, main_light_sum, main_first, main_last): (i64, i64, Option<i64>, Option<i64>) =
        conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(light_count), 0), MIN(create_time), MAX(create_time)
             FROM replies WHERE euid = ?1",
            rusqlite::params![euid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;

    // 主节点最热回帖（tooltip 用）
    let mut stmt = conn.prepare(
        "SELECT content, light_count, format_time, title, quote_content
         FROM replies WHERE euid = ?1
         ORDER BY light_count DESC, create_time DESC LIMIT 3",
    )?;
    let main_top: Vec<InteractionQuote> = stmt
        .query_map(rusqlite::params![euid], |row| {
            Ok(InteractionQuote {
                content: row.get(0)?,
                light_count: row.get(1)?,
                format_time: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                title: row.get(3)?,
                quote_content: row.get(4)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    // 全部引用关系（单次查询，内存聚合）
    struct Agg {
        count: i64,
        light_sum: i64,
        first: i64,
        last: i64,
        quotes: Vec<InteractionQuote>,
    }

    let mut stmt = conn.prepare(
        "SELECT quote_username, content, light_count, format_time, title, quote_content, create_time
         FROM replies
         WHERE euid = ?1 AND quote_username IS NOT NULL AND quote_username != ''
         ORDER BY create_time DESC",
    )?;
    let rows = stmt.query_map(rusqlite::params![euid], |row| {
        Ok((
            row.get::<_, String>(0)?,
            InteractionQuote {
                content: row.get(1)?,
                light_count: row.get(2)?,
                format_time: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                title: row.get(4)?,
                quote_content: row.get(5)?,
            },
            row.get::<_, i64>(6)?,
        ))
    })?;

    let mut map: HashMap<String, Agg> = HashMap::new();
    let mut total_interactions: i64 = 0;
    for row in rows {
        let (target, quote, ts) = row?;
        // 跳过引用自己的情况（自环）和系统统称（如小黑屋住户）
        if target == main_username || COLLECTIVE_NAMES.contains(&target.as_str()) {
            continue;
        }
        total_interactions += 1;
        let agg = map.entry(target).or_insert(Agg {
            count: 0,
            light_sum: 0,
            first: ts,
            last: ts,
            quotes: Vec::new(),
        });
        agg.count += 1;
        agg.light_sum += quote.light_count;
        if ts < agg.first {
            agg.first = ts;
        }
        if ts > agg.last {
            agg.last = ts;
        }
        agg.quotes.push(quote);
    }

    // 按互动次数排序，截断前 max_nodes 名
    let mut targets: Vec<(String, Agg)> = map.into_iter().collect();
    targets.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(b.1.light_sum.cmp(&a.1.light_sum)));
    let total_targets = targets.len() as i64;
    targets.truncate(max_nodes);

    // 每个目标的 top quotes 按点亮数取前 3
    let mut nodes = Vec::with_capacity(targets.len() + 1);
    let mut edges = Vec::with_capacity(targets.len());
    for (name, mut agg) in targets {
        agg.quotes
            .sort_by(|a, b| b.light_count.cmp(&a.light_count).then(b.format_time.cmp(&a.format_time)));
        agg.quotes.truncate(3);
        let node = InteractionNode {
            first_time: fmt_ts(agg.first),
            last_time: fmt_ts(agg.last),
            count: agg.count,
            light_sum: agg.light_sum,
            top_quotes: agg.quotes.clone(),
            name: name.clone(),
            is_main: false,
        };
        edges.push(InteractionEdge {
            source: main_username.clone(),
            target: name,
            count: agg.count,
            light_sum: agg.light_sum,
            top_quotes: agg.quotes,
        });
        nodes.push(node);
    }

    // 主节点
    nodes.push(InteractionNode {
        name: main_username.clone(),
        is_main: true,
        count: main_count,
        light_sum: main_light_sum,
        first_time: main_first.map(fmt_ts).unwrap_or_default(),
        last_time: main_last.map(fmt_ts).unwrap_or_default(),
        top_quotes: main_top,
    });

    Ok(InteractionGraph {
        main_username,
        total_interactions,
        total_targets,
        shown_targets: edges.len(),
        nodes,
        edges,
    })
}

/// 分页查询某用户与指定对象的全部互动回帖（点击节点后的详情面板）。
/// 返回（总数, 当前页数据）。
pub fn query_interaction_detail(
    conn: &Connection,
    euid: &str,
    target: &str,
    limit: usize,
    offset: usize,
) -> Result<(i64, Vec<ReplyRow>)> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM replies
         WHERE euid = ?1 AND quote_username = ?2",
        rusqlite::params![euid, target],
        |row| row.get(0),
    )?;

    let mut stmt = conn.prepare(
        "SELECT pid, tid, puid, euid, username, content,
                quote, quote_pid, quote_tid, quote_puid, quote_euid,
                quote_username, quote_content, quote_create_time,
                create_time, light_count, unlight_count,
                title, topic_id, topic_name, format_time
         FROM replies
         WHERE euid = ?1 AND quote_username = ?2
         ORDER BY create_time DESC LIMIT ?3 OFFSET ?4",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![euid, target, limit as i64, offset as i64],
        row_to_reply,
    )?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok((total, result))
}

// ── Posts ──

pub fn upsert_posts(conn: &Connection, posts: &[PostRow]) -> Result<usize> {
    let now = chrono::Utc::now().timestamp();
    let tx = conn.unchecked_transaction()?;

    for p in posts {
        tx.execute(
            "INSERT INTO posts (
                tid, euid, username, title, summary,
                create_time, lastpost_time, replies, visits, lights,
                recommend_num, forum_name, topic_name, topic_id,
                total_pics, has_video, share_num, format_time, fetched_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, ?8, ?9, ?10,
                ?11, ?12, ?13, ?14,
                ?15, ?16, ?17, ?18, ?19
            )
            ON CONFLICT(tid) DO UPDATE SET
                euid = excluded.euid,
                username = excluded.username,
                title = excluded.title,
                summary = excluded.summary,
                create_time = excluded.create_time,
                lastpost_time = excluded.lastpost_time,
                replies = excluded.replies,
                visits = excluded.visits,
                lights = excluded.lights,
                recommend_num = excluded.recommend_num,
                forum_name = excluded.forum_name,
                topic_name = excluded.topic_name,
                topic_id = excluded.topic_id,
                total_pics = excluded.total_pics,
                has_video = excluded.has_video,
                share_num = excluded.share_num,
                format_time = excluded.format_time,
                fetched_at = excluded.fetched_at
                -- 注意: ai_analyzed 不在更新列表 → 重抓保留已分析标记",
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

pub fn save_ai_post_analysis(conn: &Connection, euid: &str, result_json: &str, analyzed_until: i64) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO ai_post_analysis (euid, result, created_at, updated_at, analyzed_until)
         VALUES (?1, ?2, ?3, ?3, ?4)
         ON CONFLICT(euid) DO UPDATE SET
            result = excluded.result,
            updated_at = excluded.updated_at,
            analyzed_until = excluded.analyzed_until",
        rusqlite::params![euid, result_json, now, analyzed_until],
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

/// 该用户回帖数据中最大的 fetched_at（无数据为 0）。
/// 写入 ai_analysis.analyzed_until 作为兼容性元数据，不参与增量判断。
pub fn max_replies_fetched_at(conn: &Connection, euid: &str) -> Result<i64> {
    let v: i64 = conn.query_row(
        "SELECT COALESCE(MAX(fetched_at), 0) FROM replies WHERE euid = ?1",
        rusqlite::params![euid],
        |row| row.get(0),
    )?;
    Ok(v)
}

/// 该用户发帖数据中最大的 fetched_at（无数据为 0）。
pub fn max_posts_fetched_at(conn: &Connection, euid: &str) -> Result<i64> {
    let v: i64 = conn.query_row(
        "SELECT COALESCE(MAX(fetched_at), 0) FROM posts WHERE euid = ?1",
        rusqlite::params![euid],
        |row| row.get(0),
    )?;
    Ok(v)
}

pub fn save_ai_analysis(conn: &Connection, euid: &str, result_json: &str, analyzed_until: i64) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO ai_analysis (euid, result, created_at, updated_at, analyzed_until)
         VALUES (?1, ?2, ?3, ?3, ?4)
         ON CONFLICT(euid) DO UPDATE SET
            result = excluded.result,
            updated_at = excluded.updated_at,
            analyzed_until = excluded.analyzed_until",
        rusqlite::params![euid, result_json, now, analyzed_until],
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

pub fn save_ghost_profile(conn: &Connection, euid: &str, result_json: &str) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT OR REPLACE INTO ghost_profile (euid, result, created_at, updated_at)
         VALUES (?1, ?2, COALESCE((SELECT created_at FROM ghost_profile WHERE euid = ?1), ?3), ?3)",
        rusqlite::params![euid, result_json, now],
    )?;
    Ok(())
}

pub fn get_ghost_profile(conn: &Connection, euid: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT result FROM ghost_profile WHERE euid = ?1")?;
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

pub fn get_username(conn: &Connection, euid: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT username FROM replies WHERE euid = ? LIMIT 1"
    )?;
    let result: Option<String> = stmt.query_row(rusqlite::params![euid], |row| {
        row.get(0)
    }).ok();
    if result.is_some() {
        return Ok(result);
    }

    let mut stmt = conn.prepare(
        "SELECT username FROM posts WHERE euid = ? LIMIT 1"
    )?;
    let result: Option<String> = stmt.query_row(rusqlite::params![euid], |row| {
        row.get(0)
    }).ok();
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

pub fn query_topic_distribution(
    conn: &Connection,
    euid: &str,
) -> Result<HashMap<String, usize>> {
    let mut stmt = conn.prepare(
        "SELECT topic_name, COUNT(*) as cnt FROM replies WHERE euid = ? AND topic_name IS NOT NULL GROUP BY topic_name ORDER BY cnt DESC",
    )?;

    let mut dist = HashMap::new();
    let rows = stmt.query_map(rusqlite::params![euid], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
    })?;

    for row in rows {
        let (name, count) = row?;
        dist.insert(name, count);
    }
    Ok(dist)
}

pub fn query_time_distribution(
    conn: &Connection,
    euid: &str,
) -> Result<BTreeMap<String, usize>> {
    let mut stmt = conn.prepare(
        "SELECT strftime('%Y-%m', create_time, 'unixepoch') as month, COUNT(*) as cnt
         FROM replies WHERE euid = ? GROUP BY month ORDER BY month",
    )?;

    let mut dist = BTreeMap::new();
    let rows = stmt.query_map(rusqlite::params![euid], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
    })?;

    for row in rows {
        let (month, count) = row?;
        dist.insert(month, count);
    }
    Ok(dist)
}

// ── Keyword search for Q&A ──

/// Search replies by keywords, filtered by euid and optional topics.
pub fn search_replies(
    conn: &Connection,
    euid: &str,
    keywords: &[String],
    topic_filter: &[String],
    sort_by: &str,
    max_results: usize,
) -> Result<Vec<ReplyRow>> {
    if keywords.is_empty() {
        return Ok(Vec::new());
    }

    let mut conditions = vec!["euid = ?".to_string()];
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(euid.to_string())];

    // Build keyword conditions (OR across content and title)
    let mut kw_conds = Vec::new();
    for kw in keywords {
        kw_conds.push("(content LIKE ? OR title LIKE ?)".to_string());
        let pattern = format!("%{}%", kw);
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern));
    }
    if !kw_conds.is_empty() {
        conditions.push(format!("({})", kw_conds.join(" OR ")));
    }

    // Topic filter
    if !topic_filter.is_empty() {
        let mut topic_conds = Vec::new();
        for topic in topic_filter {
            topic_conds.push("topic_name LIKE ?".to_string());
            params.push(Box::new(format!("%{}%", topic)));
        }
        if !topic_conds.is_empty() {
            conditions.push(format!("({})", topic_conds.join(" OR ")));
        }
    }

    let order = match sort_by {
        "create_time" => "create_time DESC",
        "light_count" => "light_count DESC",
        _ => "create_time DESC", // default for keyword search
    };

    let sql = format!(
        "SELECT pid, tid, puid, euid, username, content,
                quote, quote_pid, quote_tid, quote_puid, quote_euid,
                quote_username, quote_content, quote_create_time,
                create_time, light_count, unlight_count,
                title, topic_id, topic_name, format_time
         FROM replies WHERE {} ORDER BY {} LIMIT ?",
        conditions.join(" AND "), order
    );
    params.push(Box::new(max_results as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(param_refs.as_slice(), row_to_reply)?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Search posts by keywords, filtered by euid and optional topics.
pub fn search_posts(
    conn: &Connection,
    euid: &str,
    keywords: &[String],
    topic_filter: &[String],
    sort_by: &str,
    max_results: usize,
) -> Result<Vec<PostRow>> {
    if keywords.is_empty() {
        return Ok(Vec::new());
    }

    let mut conditions = vec!["euid = ?".to_string()];
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(euid.to_string())];

    // Build keyword conditions
    let mut kw_conds = Vec::new();
    for kw in keywords {
        kw_conds.push("(title LIKE ? OR summary LIKE ?)".to_string());
        let pattern = format!("%{}%", kw);
        params.push(Box::new(pattern.clone()));
        params.push(Box::new(pattern));
    }
    if !kw_conds.is_empty() {
        conditions.push(format!("({})", kw_conds.join(" OR ")));
    }

    // Topic filter
    if !topic_filter.is_empty() {
        let mut topic_conds = Vec::new();
        for topic in topic_filter {
            topic_conds.push("(topic_name LIKE ? OR forum_name LIKE ?)".to_string());
            let pattern = format!("%{}%", topic);
            params.push(Box::new(pattern.clone()));
            params.push(Box::new(pattern));
        }
        if !topic_conds.is_empty() {
            conditions.push(format!("({})", topic_conds.join(" OR ")));
        }
    }

    let order = match sort_by {
        "create_time" => "create_time DESC",
        "light_count" | "lights" => "lights DESC",
        "replies" => "replies DESC",
        _ => "create_time DESC",
    };

    let sql = format!(
        "SELECT tid, euid, username, title, summary,
                create_time, lastpost_time, replies, visits, lights,
                recommend_num, forum_name, topic_name, topic_id,
                total_pics, has_video, share_num, format_time
         FROM posts WHERE {} ORDER BY {} LIMIT ?",
        conditions.join(" AND "), order
    );
    params.push(Box::new(max_results as i64));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(param_refs.as_slice(), row_to_post)?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Query post topic distribution for a given euid.
pub fn query_post_topic_distribution(
    conn: &Connection,
    euid: &str,
) -> Result<HashMap<String, usize>> {
    let mut stmt = conn.prepare(
        "SELECT topic_name, COUNT(*) as cnt FROM posts WHERE euid = ? AND topic_name IS NOT NULL AND topic_name != '' GROUP BY topic_name ORDER BY cnt DESC",
    )?;

    let mut dist = HashMap::new();
    let rows = stmt.query_map(rusqlite::params![euid], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
    })?;

    for row in rows {
        let (name, count) = row?;
        dist.insert(name, count);
    }
    Ok(dist)
}

/// Search replies by time range (YYYY-MM format), filtered by euid.
pub fn search_replies_by_time(
    conn: &Connection,
    euid: &str,
    start_date: &str,
    end_date: &str,
    max_results: usize,
) -> Result<Vec<ReplyRow>> {
    let sql = format!(
        "SELECT pid, tid, puid, euid, username, content,
                quote, quote_pid, quote_tid, quote_puid, quote_euid,
                quote_username, quote_content, quote_create_time,
                create_time, light_count, unlight_count,
                title, topic_id, topic_name, format_time
         FROM replies WHERE euid = ? AND strftime('%Y-%m', create_time, 'unixepoch') BETWEEN ? AND ?
         ORDER BY create_time DESC LIMIT ?"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![euid, start_date, end_date, max_results as i64],
        row_to_reply,
    )?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Search posts by time range (YYYY-MM format), filtered by euid.
pub fn search_posts_by_time(
    conn: &Connection,
    euid: &str,
    start_date: &str,
    end_date: &str,
    max_results: usize,
) -> Result<Vec<PostRow>> {
    let sql = format!(
        "SELECT tid, euid, username, title, summary,
                create_time, lastpost_time, replies, visits, lights,
                recommend_num, forum_name, topic_name, topic_id,
                total_pics, has_video, share_num, format_time
         FROM posts WHERE euid = ? AND strftime('%Y-%m', create_time, 'unixepoch') BETWEEN ? AND ?
         ORDER BY create_time DESC LIMIT ?"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![euid, start_date, end_date, max_results as i64],
        row_to_post,
    )?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Get hot replies sorted by light_count.
pub fn get_hot_replies(
    conn: &Connection,
    euid: &str,
    sort_by: &str,
    limit: usize,
) -> Result<Vec<ReplyRow>> {
    let order = match sort_by {
        "light_count" => "light_count DESC",
        _ => "light_count DESC",
    };
    let sql = format!(
        "SELECT pid, tid, puid, euid, username, content,
                quote, quote_pid, quote_tid, quote_puid, quote_euid,
                quote_username, quote_content, quote_create_time,
                create_time, light_count, unlight_count,
                title, topic_id, topic_name, format_time
         FROM replies WHERE euid = ? ORDER BY {} LIMIT ?",
        order
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![euid, limit as i64], row_to_reply)?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Get hot posts sorted by lights, replies, or visits.
pub fn get_hot_posts(
    conn: &Connection,
    euid: &str,
    sort_by: &str,
    limit: usize,
) -> Result<Vec<PostRow>> {
    let order = match sort_by {
        "lights" => "lights DESC",
        "replies" => "replies DESC",
        "visits" => "visits DESC",
        _ => "lights DESC",
    };
    let sql = format!(
        "SELECT tid, euid, username, title, summary,
                create_time, lastpost_time, replies, visits, lights,
                recommend_num, forum_name, topic_name, topic_id,
                total_pics, has_video, share_num, format_time
         FROM posts WHERE euid = ? ORDER BY {} LIMIT ?",
        order
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![euid, limit as i64], row_to_post)?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

// ── Monitor: 分区舆论监控 ──

use crate::topic::{Post, PostReply};

/// Upsert monitor posts (from topic::Post)
pub fn upsert_monitor_posts(conn: &Connection, topic_id: &str, posts: &[Post]) -> Result<usize> {
    let now = chrono::Utc::now().timestamp();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let tx = conn.unchecked_transaction()?;

    for p in posts {
        // Try to parse create_time string into Unix timestamp
        let ts = p.create_time.as_deref()
            .and_then(|t| parse_post_time_to_ts(t))
            .unwrap_or(now); // fallback to current time

        tx.execute(
            "INSERT OR IGNORE INTO monitor_posts (
                tid, topic_id, title, author, reply_count, light_count,
                create_time, format_time, fetched_at, fetch_date
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                p.tid.parse::<i64>().unwrap_or(0),
                topic_id,
                p.title,
                p.author.as_deref().unwrap_or(""),
                p.reply_count.unwrap_or(0),
                p.light_count.unwrap_or(0),
                ts,
                p.create_time.as_deref(),
                now,
                today,
            ],
        )?;
    }

    tx.commit()?;
    Ok(posts.len())
}

/// Parse a post time string like "06-02 20:30" or "2026-06-02 20:30" to Unix timestamp
pub fn parse_post_time_to_ts(s: &str) -> Option<i64> {
    let now = chrono::Utc::now();
    let today = now.date_naive();

    // "2026-06-02 20:30"
    if s.len() >= 16 {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&s[..16], "%Y-%m-%d %H:%M") {
            return Some(dt.and_utc().timestamp());
        }
    }
    // "2026-06-02"
    if s.len() >= 10 {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(&s[..10], "%Y-%m-%d") {
            return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp());
        }
    }
    // "06-02 20:30"
    if s.len() >= 11 && &s[2..3] == "-" {
        let year = today.format("%Y").to_string();
        let full = format!("{}-{}", year, &s[..11]);
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&full, "%Y-%m-%d %H:%M") {
            return Some(dt.and_utc().timestamp());
        }
    }
    // "06-02"
    if s.len() >= 5 && &s[2..3] == "-" {
        let year = today.format("%Y").to_string();
        let full = format!("{}-{}", year, &s[..5]);
        if let Ok(d) = chrono::NaiveDate::parse_from_str(&full, "%Y-%m-%d") {
            return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp());
        }
    }
    None
}

/// Upsert monitor replies (from topic::PostReply)
pub fn upsert_monitor_replies(conn: &Connection, topic_id: &str, tid: i64, replies: &[PostReply]) -> Result<usize> {
    let now = chrono::Utc::now().timestamp();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let tx = conn.unchecked_transaction()?;

    for r in replies {
        tx.execute(
            "INSERT OR IGNORE INTO monitor_replies (
                pid, tid, topic_id, username, content, light_count,
                create_time, format_time, fetched_at, fetch_date
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                r.pid.parse::<i64>().unwrap_or(0),
                tid,
                topic_id,
                r.username,
                r.content,
                r.light_count,
                0i64,
                r.create_time.as_deref(),
                now,
                today,
            ],
        )?;
    }

    tx.commit()?;
    Ok(replies.len())
}

/// Query monitor posts by topic_id, optionally filtered by date
pub fn query_monitor_posts(
    conn: &Connection,
    topic_id: &str,
    date: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<Vec<serde_json::Value>> {
    let sql = match date {
        Some(_) => format!(
            "SELECT tid, title, author, reply_count, light_count, create_time, format_time, fetch_date
             FROM monitor_posts WHERE topic_id = ? AND fetch_date = ? ORDER BY light_count DESC LIMIT ? OFFSET ?"
        ),
        None => format!(
            "SELECT tid, title, author, reply_count, light_count, create_time, format_time, fetch_date
             FROM monitor_posts WHERE topic_id = ? ORDER BY light_count DESC LIMIT ? OFFSET ?"
        ),
    };

    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<serde_json::Value> = match date {
        Some(d) => {
            stmt.query_map(rusqlite::params![topic_id, d, limit as i64, offset as i64], |row| {
                Ok(serde_json::json!({
                    "tid": row.get::<_, i64>(0)?,
                    "title": row.get::<_, String>(1)?,
                    "author": row.get::<_, String>(2)?,
                    "reply_count": row.get::<_, i64>(3)?,
                    "light_count": row.get::<_, i64>(4)?,
                    "create_time": row.get::<_, i64>(5)?,
                    "format_time": row.get::<_, Option<String>>(6)?,
                    "fetch_date": row.get::<_, String>(7)?,
                }))
            })?.filter_map(|r| r.ok()).collect()
        }
        None => {
            stmt.query_map(rusqlite::params![topic_id, limit as i64, offset as i64], |row| {
                Ok(serde_json::json!({
                    "tid": row.get::<_, i64>(0)?,
                    "title": row.get::<_, String>(1)?,
                    "author": row.get::<_, String>(2)?,
                    "reply_count": row.get::<_, i64>(3)?,
                    "light_count": row.get::<_, i64>(4)?,
                    "create_time": row.get::<_, i64>(5)?,
                    "format_time": row.get::<_, Option<String>>(6)?,
                    "fetch_date": row.get::<_, String>(7)?,
                }))
            })?.filter_map(|r| r.ok()).collect()
        }
    };

    Ok(rows)
}

/// Query monitor replies by topic_id, optionally filtered by date
pub fn query_monitor_replies(
    conn: &Connection,
    topic_id: &str,
    date: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<Vec<serde_json::Value>> {
    let sql = match date {
        Some(_) => format!(
            "SELECT pid, tid, username, content, light_count, create_time, format_time, fetch_date
             FROM monitor_replies WHERE topic_id = ? AND fetch_date = ? ORDER BY light_count DESC LIMIT ? OFFSET ?"
        ),
        None => format!(
            "SELECT pid, tid, username, content, light_count, create_time, format_time, fetch_date
             FROM monitor_replies WHERE topic_id = ? ORDER BY light_count DESC LIMIT ? OFFSET ?"
        ),
    };

    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<serde_json::Value> = match date {
        Some(d) => {
            stmt.query_map(rusqlite::params![topic_id, d, limit as i64, offset as i64], |row| {
                Ok(serde_json::json!({
                    "pid": row.get::<_, i64>(0)?,
                    "tid": row.get::<_, i64>(1)?,
                    "username": row.get::<_, String>(2)?,
                    "content": row.get::<_, String>(3)?,
                    "light_count": row.get::<_, i64>(4)?,
                    "create_time": row.get::<_, i64>(5)?,
                    "format_time": row.get::<_, Option<String>>(6)?,
                    "fetch_date": row.get::<_, String>(7)?,
                }))
            })?.filter_map(|r| r.ok()).collect()
        }
        None => {
            stmt.query_map(rusqlite::params![topic_id, limit as i64, offset as i64], |row| {
                Ok(serde_json::json!({
                    "pid": row.get::<_, i64>(0)?,
                    "tid": row.get::<_, i64>(1)?,
                    "username": row.get::<_, String>(2)?,
                    "content": row.get::<_, String>(3)?,
                    "light_count": row.get::<_, i64>(4)?,
                    "create_time": row.get::<_, i64>(5)?,
                    "format_time": row.get::<_, Option<String>>(6)?,
                    "fetch_date": row.get::<_, String>(7)?,
                }))
            })?.filter_map(|r| r.ok()).collect()
        }
    };

    Ok(rows)
}

/// Count monitor posts for a topic_id, optionally filtered by date
pub fn count_monitor_posts(conn: &Connection, topic_id: &str, date: Option<&str>) -> Result<i64> {
    let tid = topic_id.to_string();
    let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match date {
        Some(_) => (
            "SELECT COUNT(*) FROM monitor_posts WHERE topic_id = ?1 AND fetch_date = ?2",
            vec![Box::new(tid), Box::new(date.unwrap().to_string())],
        ),
        None => (
            "SELECT COUNT(*) FROM monitor_posts WHERE topic_id = ?1",
            vec![Box::new(tid)],
        ),
    };
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let count: i64 = conn.query_row(sql, param_refs.as_slice(), |row| row.get(0))?;
    Ok(count)
}

/// Count monitor replies for a topic_id, optionally filtered by date
pub fn count_monitor_replies(conn: &Connection, topic_id: &str, date: Option<&str>) -> Result<i64> {
    let tid = topic_id.to_string();
    let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match date {
        Some(_) => (
            "SELECT COUNT(*) FROM monitor_replies WHERE topic_id = ?1 AND fetch_date = ?2",
            vec![Box::new(tid), Box::new(date.unwrap().to_string())],
        ),
        None => (
            "SELECT COUNT(*) FROM monitor_replies WHERE topic_id = ?1",
            vec![Box::new(tid)],
        ),
    };
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let count: i64 = conn.query_row(sql, param_refs.as_slice(), |row| row.get(0))?;
    Ok(count)
}

/// Get daily post counts for a topic over the last N days
pub fn monitor_daily_post_counts(conn: &Connection, topic_id: &str, days: i64) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT fetch_date, COUNT(*) as cnt FROM monitor_posts
         WHERE topic_id = ?1
         GROUP BY fetch_date ORDER BY fetch_date DESC LIMIT ?2"
    )?;
    let rows: Vec<serde_json::Value> = stmt.query_map(rusqlite::params![topic_id, days], |row| {
        Ok(serde_json::json!({
            "date": row.get::<_, String>(0)?,
            "count": row.get::<_, i64>(1)?,
        }))
    })?.filter_map(|r| r.ok()).collect();
    Ok(rows)
}

/// Get daily reply counts for a topic over the last N days
pub fn monitor_daily_reply_counts(conn: &Connection, topic_id: &str, days: i64) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT fetch_date, COUNT(*) as cnt FROM monitor_replies
         WHERE topic_id = ?1
         GROUP BY fetch_date ORDER BY fetch_date DESC LIMIT ?2"
    )?;
    let rows: Vec<serde_json::Value> = stmt.query_map(rusqlite::params![topic_id, days], |row| {
        Ok(serde_json::json!({
            "date": row.get::<_, String>(0)?,
            "count": row.get::<_, i64>(1)?,
        }))
    })?.filter_map(|r| r.ok()).collect();
    Ok(rows)
}

/// Save a monitor snapshot (AI analysis result)
pub fn save_monitor_snapshot(
    conn: &Connection,
    topic_id: &str,
    snapshot_date: &str,
    post_count: i64,
    reply_count: i64,
    sentiment_dist: &str,
    top_keywords: &str,
    ai_summary: &str,
    ai_raw_json: &str,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT OR REPLACE INTO monitor_snapshots
         (topic_id, snapshot_date, post_count, reply_count, sentiment_dist, top_keywords, ai_summary, ai_raw_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![topic_id, snapshot_date, post_count, reply_count, sentiment_dist, top_keywords, ai_summary, ai_raw_json, now],
    )?;
    Ok(())
}

/// Get monitor snapshots for a topic over the last N days (for trend charts)
pub fn get_monitor_snapshots(conn: &Connection, topic_id: &str, days: i64) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT topic_id, snapshot_date, post_count, reply_count, sentiment_dist, top_keywords, ai_summary, ai_raw_json, created_at
         FROM monitor_snapshots WHERE topic_id = ?1 ORDER BY snapshot_date DESC LIMIT ?2"
    )?;
    let rows: Vec<serde_json::Value> = stmt.query_map(rusqlite::params![topic_id, days], |row| {
        Ok(serde_json::json!({
            "topic_id": row.get::<_, String>(0)?,
            "snapshot_date": row.get::<_, String>(1)?,
            "post_count": row.get::<_, i64>(2)?,
            "reply_count": row.get::<_, i64>(3)?,
            "sentiment_dist": row.get::<_, Option<String>>(4)?,
            "top_keywords": row.get::<_, Option<String>>(5)?,
            "ai_summary": row.get::<_, Option<String>>(6)?,
            "ai_raw_json": row.get::<_, Option<String>>(7)?,
            "created_at": row.get::<_, i64>(8)?,
        }))
    })?.filter_map(|r| r.ok()).collect();
    Ok(rows)
}

/// Get all known TIDs for a topic (for dedup — skip already-fetched posts)
pub fn get_monitor_known_tids(conn: &Connection, topic_id: &str) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT tid FROM monitor_posts WHERE topic_id = ?1"
    )?;
    let rows: Vec<i64> = stmt
        .query_map(rusqlite::params![topic_id], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Get all distinct fetch_dates already covered for a topic (for dedup)
pub fn get_monitor_covered_dates(conn: &Connection, topic_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT fetch_date FROM monitor_posts WHERE topic_id = ?1 ORDER BY fetch_date DESC"
    )?;
    let rows: Vec<String> = stmt
        .query_map(rusqlite::params![topic_id], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Get all distinct topic_ids in monitor data (for the dropdown)
pub fn get_monitor_topics(conn: &Connection) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT topic_id FROM monitor_posts ORDER BY topic_id"
    )?;
    let rows: Vec<serde_json::Value> = stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "topic_id": row.get::<_, String>(0)?,
        }))
    })?.filter_map(|r| r.ok()).collect();
    Ok(rows)
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use rusqlite::Connection;

    fn in_memory_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        create_tables(&conn).unwrap();
        conn
    }

    fn quoted_reply(pid: i64, target: &str, light: i64, ts: i64) -> ReplyRow {
        ReplyRow {
            pid,
            tid: 100 + pid,
            puid: Some(1000),
            euid: Some("me".into()),
            username: "我".into(),
            content: format!("回复{}的内容{}", target, pid),
            quote: 1,
            quote_pid: Some(pid * 10),
            quote_tid: Some(100 + pid),
            quote_puid: Some(2000),
            quote_euid: None,
            quote_username: Some(target.into()),
            quote_content: Some(format!("{}的原文", target)),
            quote_create_time: Some(ts - 100),
            create_time: ts,
            light_count: light,
            unlight_count: 0,
            title: "帖子".into(),
            topic_id: None,
            topic_name: None,
            format_time: Some("2024-01-01 12:00".into()),
        }
    }

    #[test]
    fn interaction_graph_aggregates_by_target() {
        let conn = in_memory_conn();
        let replies = vec![
            quoted_reply(1, "甲", 10, 1700000100),
            quoted_reply(2, "甲", 30, 1700000200),
            quoted_reply(3, "甲", 5, 1700000300),
            quoted_reply(4, "乙", 99, 1700000400),
            quoted_reply(5, "我", 1, 1700000500), // 引用自己 → 跳过
            quoted_reply(6, "小黑屋住户", 20, 1700000600), // 系统统称 → 跳过
        ];
        upsert_replies(&conn, &replies).unwrap();

        let g = query_interaction_graph(&conn, "me", 10).unwrap();
        assert_eq!(g.main_username, "我");
        assert_eq!(g.total_interactions, 4);
        assert_eq!(g.total_targets, 2);
        assert_eq!(g.edges.len(), 2);

        // 按互动次数排序：甲在前
        let a = &g.nodes[0];
        assert_eq!(a.name, "甲");
        assert_eq!(a.count, 3);
        assert_eq!(a.light_sum, 45);
        assert_eq!(a.is_main, false);
        // top quotes 按点亮数取前 3：30, 10, 5
        assert_eq!(a.top_quotes.len(), 3);
        assert_eq!(a.top_quotes[0].light_count, 30);

        let b = &g.nodes[1];
        assert_eq!(b.name, "乙");
        assert_eq!(b.count, 1);
        assert_eq!(b.light_sum, 99);

        // 主节点（统计所有回帖，包括被过滤掉的统称引用）
        let main = g.nodes.iter().find(|n| n.is_main).unwrap();
        assert_eq!(main.count, 6);
        assert_eq!(main.light_sum, 165);

        // 边方向
        let edge = &g.edges[0];
        assert_eq!(edge.source, "我");
        assert_eq!(edge.target, "甲");
        assert_eq!(edge.count, 3);
    }

    #[test]
    fn interaction_graph_truncates_nodes() {
        let conn = in_memory_conn();
        let mut replies = Vec::new();
        for i in 1..=5 {
            replies.push(quoted_reply(i, &format!("用户{}", i), i, 1700000000 + i));
        }
        upsert_replies(&conn, &replies).unwrap();

        let g = query_interaction_graph(&conn, "me", 3).unwrap();
        assert_eq!(g.total_targets, 5);
        assert_eq!(g.shown_targets, 3);
        assert_eq!(g.edges.len(), 3);
        assert_eq!(g.nodes.len(), 4); // 3 目标 + 主节点
    }

    #[test]
    fn interaction_detail_paginates() {
        let conn = in_memory_conn();
        let mut replies = Vec::new();
        for i in 1..=5 {
            replies.push(quoted_reply(i, "甲", i, 1700000000 + i));
        }
        upsert_replies(&conn, &replies).unwrap();

        let (total, page) = query_interaction_detail(&conn, "me", "甲", 2, 0).unwrap();
        assert_eq!(total, 5);
        assert_eq!(page.len(), 2);
        // 按时间倒序：最新在前
        assert_eq!(page[0].pid, 5);

        let (_, page2) = query_interaction_detail(&conn, "me", "甲", 2, 4).unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].pid, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn in_memory_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        create_tables(&conn).unwrap();
        conn
    }

    fn sample_reply(pid: i64, tid: i64) -> ReplyRow {
        ReplyRow {
            pid,
            tid,
            puid: Some(1000),
            euid: Some("test_euid".into()),
            username: "测试用户".into(),
            content: "这是一条测试回帖".into(),
            quote: 0,
            quote_pid: None,
            quote_tid: None,
            quote_puid: None,
            quote_euid: None,
            quote_username: None,
            quote_content: None,
            quote_create_time: None,
            create_time: 1700000000,
            light_count: 5,
            unlight_count: 1,
            title: "测试帖子标题".into(),
            topic_id: Some(1),
            topic_name: Some("步行街".into()),
            format_time: Some("01-01 00:00".into()),
        }
    }

    fn sample_post(tid: i64) -> PostRow {
        PostRow {
            tid,
            euid: "test_euid".into(),
            username: "测试用户".into(),
            title: "测试帖子".into(),
            summary: "帖子摘要".into(),
            create_time: 1700000000,
            lastpost_time: 1700000100,
            replies: 10,
            visits: 100,
            lights: 5,
            recommend_num: 1,
            forum_name: "步行街".into(),
            topic_name: "步行街".into(),
            topic_id: 1,
            total_pics: 0,
            has_video: false,
            share_num: 0,
            format_time: Some("2024-01-01 00:00".into()),
        }
    }

    // ── open_db / create_tables ──

    #[test]
    fn open_db_creates_tables() {
        let conn = in_memory_conn();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='replies'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(count > 0);
    }

    // ── upsert_replies ──

    #[test]
    fn upsert_replies_inserts() {
        let conn = in_memory_conn();
        let replies = vec![sample_reply(1, 100), sample_reply(2, 200)];
        let count = upsert_replies(&conn, &replies).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn upsert_replies_overwrites_existing() {
        let conn = in_memory_conn();
        upsert_replies(&conn, &[sample_reply(1, 100)]).unwrap();

        let mut updated = sample_reply(1, 100);
        updated.content = "更新后的内容".into();
        updated.light_count = 99;
        upsert_replies(&conn, &[updated]).unwrap();

        let rows = query_replies(&conn, Some("test_euid"), 10, 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "更新后的内容");
        assert_eq!(rows[0].light_count, 99);
    }

    #[test]
    fn upsert_replies_empty_slice_ok() {
        let conn = in_memory_conn();
        let count = upsert_replies(&conn, &[]).unwrap();
        assert_eq!(count, 0);
    }

    // ── query_replies ──

    #[test]
    fn query_replies_by_euid() {
        let conn = in_memory_conn();
        let mut r1 = sample_reply(1, 100);
        r1.euid = Some("user_a".into());
        let mut r2 = sample_reply(2, 200);
        r2.euid = Some("user_b".into());
        upsert_replies(&conn, &[r1, r2]).unwrap();

        let rows = query_replies(&conn, Some("user_a"), 10, 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 1);
    }

    #[test]
    fn query_replies_all_users() {
        let conn = in_memory_conn();
        let r1 = sample_reply(1, 100);
        let r2 = sample_reply(2, 200);
        upsert_replies(&conn, &[r1, r2]).unwrap();

        let rows = query_replies(&conn, None, 10, 0).unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn query_replies_limit_and_offset() {
        let conn = in_memory_conn();
        let replies: Vec<_> = (1..=5).map(|i| sample_reply(i, i * 100)).collect();
        upsert_replies(&conn, &replies).unwrap();

        let rows = query_replies(&conn, None, 2, 1).unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn query_replies_returns_empty_for_unknown_euid() {
        let conn = in_memory_conn();
        let rows = query_replies(&conn, Some("unknown"), 10, 0).unwrap();
        assert!(rows.is_empty());
    }

    // ── count_replies ──

    #[test]
    fn count_replies_total() {
        let conn = in_memory_conn();
        upsert_replies(&conn, &[sample_reply(1, 100), sample_reply(2, 200)]).unwrap();
        assert_eq!(count_replies(&conn, None).unwrap(), 2);
    }

    #[test]
    fn count_replies_by_euid() {
        let conn = in_memory_conn();
        let mut r = sample_reply(1, 100);
        r.euid = Some("count_user".into());
        upsert_replies(&conn, &[r]).unwrap();

        assert_eq!(count_replies(&conn, Some("count_user")).unwrap(), 1);
        assert_eq!(count_replies(&conn, Some("other")).unwrap(), 0);
    }

    #[test]
    fn count_replies_empty_db() {
        let conn = in_memory_conn();
        assert_eq!(count_replies(&conn, None).unwrap(), 0);
    }

    // ── row_to_reply (implicitly tested by query_replies, but explicit too) ──

    #[test]
    fn query_replies_has_all_fields() {
        let conn = in_memory_conn();
        let r = sample_reply(1, 100);
        upsert_replies(&conn, &[r.clone()]).unwrap();

        let rows = query_replies(&conn, None, 1, 0).unwrap();
        let row = &rows[0];
        assert_eq!(row.pid, r.pid);
        assert_eq!(row.tid, r.tid);
        assert_eq!(row.content, r.content);
        assert_eq!(row.light_count, r.light_count);
        assert_eq!(row.title, r.title);
    }

    // ── upsert_posts ──

    #[test]
    fn upsert_posts_inserts() {
        let conn = in_memory_conn();
        let posts = vec![sample_post(1), sample_post(2)];
        let count = upsert_posts(&conn, &posts).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn upsert_posts_overwrites() {
        let conn = in_memory_conn();
        upsert_posts(&conn, &[sample_post(1)]).unwrap();

        let mut updated = sample_post(1);
        updated.title = "更新后的标题".into();
        upsert_posts(&conn, &[updated]).unwrap();

        let rows = query_posts(&conn, Some("test_euid"), 10, 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "更新后的标题");
    }

    // ── query_posts ──

    #[test]
    fn query_posts_by_euid() {
        let conn = in_memory_conn();
        let mut p1 = sample_post(1);
        p1.euid = "pa".into();
        let mut p2 = sample_post(2);
        p2.euid = "pb".into();
        upsert_posts(&conn, &[p1, p2]).unwrap();

        let rows = query_posts(&conn, Some("pa"), 10, 0).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn query_posts_limit_offset() {
        let conn = in_memory_conn();
        let posts: Vec<_> = (1..=5).map(|i| sample_post(i)).collect();
        upsert_posts(&conn, &posts).unwrap();

        let rows = query_posts(&conn, None, 3, 0).unwrap();
        assert_eq!(rows.len(), 3);
    }

    // ── count_posts ──

    #[test]
    fn count_posts_works() {
        let conn = in_memory_conn();
        upsert_posts(&conn, &[sample_post(1), sample_post(2)]).unwrap();
        assert_eq!(count_posts(&conn, None).unwrap(), 2);
    }

    // ── ai_analysis ──

    #[test]
    fn save_and_get_ai_analysis() {
        let conn = in_memory_conn();
        save_ai_analysis(&conn, "user1", r#"{"label":"正常用户"}"#, 1000).unwrap();
        let result = get_ai_analysis(&conn, "user1").unwrap();
        assert!(result.is_some());
        assert!(result.unwrap().contains("正常用户"));
        // analyzed_until 作为元数据快照写入
        let cursor: i64 = conn
            .query_row("SELECT analyzed_until FROM ai_analysis WHERE euid='user1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cursor, 1000);
    }

    #[test]
    fn get_ai_analysis_nonexistent_euid() {
        let conn = in_memory_conn();
        let result = get_ai_analysis(&conn, "no_one").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn save_ai_analysis_overwrites() {
        let conn = in_memory_conn();
        save_ai_analysis(&conn, "user1", r#"{"v":1}"#, 1000).unwrap();
        save_ai_analysis(&conn, "user1", r#"{"v":2}"#, 2000).unwrap();
        let result = get_ai_analysis(&conn, "user1").unwrap().unwrap();
        assert!(result.contains("\"v\":2"));
        // 游标快照随覆盖更新
        let cursor: i64 = conn
            .query_row("SELECT analyzed_until FROM ai_analysis WHERE euid='user1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cursor, 2000);
    }

    // ── ai_post_analysis ──

    #[test]
    fn save_and_get_ai_post_analysis() {
        let conn = in_memory_conn();
        save_ai_post_analysis(&conn, "user1", r#"{"posts":[]}"#, 1000).unwrap();
        let result = get_ai_post_analysis(&conn, "user1").unwrap();
        assert!(result.is_some());
        let cursor: i64 = conn
            .query_row("SELECT analyzed_until FROM ai_post_analysis WHERE euid='user1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cursor, 1000);
    }

    #[test]
    fn get_ai_post_analysis_nonexistent() {
        let conn = in_memory_conn();
        let result = get_ai_post_analysis(&conn, "no_one").unwrap();
        assert!(result.is_none());
    }

    // ── similarity_analysis ──

    #[test]
    fn save_and_get_similarity_analysis() {
        let conn = in_memory_conn();
        save_similarity_analysis(&conn, "user1", 0.8, r#"{"groups":[]}"#).unwrap();
        let result = get_similarity_analysis(&conn, "user1", 0.8).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn similarity_analysis_by_different_thresholds() {
        let conn = in_memory_conn();
        save_similarity_analysis(&conn, "user1", 0.5, r#"{"t":0.5}"#).unwrap();
        save_similarity_analysis(&conn, "user1", 0.9, r#"{"t":0.9}"#).unwrap();

        let r1 = get_similarity_analysis(&conn, "user1", 0.5).unwrap().unwrap();
        let r2 = get_similarity_analysis(&conn, "user1", 0.9).unwrap().unwrap();
        assert!(r1.contains("0.5"));
        assert!(r2.contains("0.9"));
    }

    // ── get_all_euids ──

    #[test]
    fn get_all_euids_from_replies_and_posts() {
        let conn = in_memory_conn();
        let mut r = sample_reply(1, 100);
        r.euid = Some("aaa".into());
        r.username = "用户A".into();
        upsert_replies(&conn, &[r]).unwrap();

        let mut p = sample_post(1);
        p.euid = "bbb".into();
        p.username = "用户B".into();
        upsert_posts(&conn, &[p]).unwrap();

        let euids = get_all_euids(&conn).unwrap();
        assert_eq!(euids.len(), 2);
        let names: Vec<&str> = euids.iter().map(|(_, n)| n.as_str()).collect();
        assert!(names.contains(&"用户A"));
        assert!(names.contains(&"用户B"));
    }

    #[test]
    fn get_all_euids_deduplicates() {
        let conn = in_memory_conn();
        let mut r = sample_reply(1, 100);
        r.euid = Some("same".into());
        r.username = "用户X".into();
        upsert_replies(&conn, &[r]).unwrap();

        let mut p = sample_post(1);
        p.euid = "same".into();
        p.username = "用户X".into();
        upsert_posts(&conn, &[p]).unwrap();

        let euids = get_all_euids(&conn).unwrap();
        assert_eq!(euids.len(), 1);
    }

    #[test]
    fn get_all_euids_empty() {
        let conn = in_memory_conn();
        let euids = get_all_euids(&conn).unwrap();
        assert!(euids.is_empty());
    }

    // ── save_batch_error ──

    #[test]
    fn save_batch_error_works() {
        let conn = in_memory_conn();
        save_batch_error(&conn, "user1", "reply", 0, "test error", Some("raw")).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_batch_errors", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn save_batch_error_without_raw() {
        let conn = in_memory_conn();
        save_batch_error(&conn, "user1", "post", 5, "some error", None).unwrap();

        let raw: Option<String> = conn
            .query_row(
                "SELECT raw_response FROM ai_batch_errors LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(raw.is_none());
    }

    // ── Row serialization round-trip ──

    #[test]
    fn reply_row_serde_roundtrip() {
        let original = sample_reply(42, 999);
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: ReplyRow = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.pid, original.pid);
        assert_eq!(deserialized.tid, original.tid);
        assert_eq!(deserialized.content, original.content);
        assert_eq!(deserialized.username, original.username);
    }

    #[test]
    fn post_row_serde_roundtrip() {
        let original = sample_post(99);
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: PostRow = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tid, original.tid);
        assert_eq!(deserialized.title, original.title);
        assert_eq!(deserialized.has_video, original.has_video);
    }
}

// ── 增量 AI 分析游标 ──

#[cfg(test)]
mod incremental_analysis_tests {
    use super::*;

    fn in_memory_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        create_tables(&conn).unwrap();
        conn
    }

    fn reply_row(pid: i64) -> ReplyRow {
        ReplyRow {
            pid,
            tid: 100 + pid,
            puid: Some(1000),
            euid: Some("u1".into()),
            username: "甲".into(),
            content: format!("内容{}", pid),
            quote: 0,
            quote_pid: None,
            quote_tid: None,
            quote_puid: None,
            quote_euid: None,
            quote_username: None,
            quote_content: None,
            quote_create_time: None,
            create_time: 1700000000 + pid,
            light_count: 0,
            unlight_count: 0,
            title: "t".into(),
            topic_id: None,
            topic_name: None,
            format_time: None,
        }
    }

    /// 插入一条"已存在缓存"记录（等效于旧版本全量分析后的状态）。
    fn insert_cached_analysis(conn: &Connection, euid: &str, updated_at: i64, analyzed_until: i64) {
        conn.execute(
            "INSERT INTO ai_analysis (euid, result, created_at, updated_at, analyzed_until)
             VALUES (?1, '{}', ?2, ?2, ?3)",
            rusqlite::params![euid, updated_at, analyzed_until],
        )
        .unwrap();
    }

    #[test]
    fn migration_adds_analyzed_until_idempotently() {
        let conn = in_memory_conn();
        // 重复迁移幂等
        add_analyzed_until_columns(&conn).unwrap();

        // 打开新连接（走 create_tables）也不报错
        let conn2 = Connection::open_in_memory().unwrap();
        create_tables(&conn2).unwrap();
        create_tables(&conn2).unwrap();

        let has = conn2
            .prepare("PRAGMA table_info(ai_analysis)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .any(|name| name == "analyzed_until");
        assert!(has, "ai_analysis.analyzed_until 列应存在");
    }

    #[test]
    fn backfill_sets_cursor_from_fetched_at() {
        let conn = in_memory_conn();
        upsert_replies(&conn, &[reply_row(1), reply_row(2)]).unwrap();
        // 模拟：数据在时间 1000 抓取，缓存分析在 2000 完成
        conn.execute("UPDATE replies SET fetched_at = 1000", []).unwrap();
        insert_cached_analysis(&conn, "u1", 2000, 0);

        backfill_analysis_cursors(&conn).unwrap();

        let cursor: i64 = conn
            .query_row("SELECT analyzed_until FROM ai_analysis WHERE euid='u1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cursor, 1000);
        // 无缓存的用户不受影响
        let none: i64 = conn
            .query_row("SELECT COALESCE((SELECT analyzed_until FROM ai_analysis WHERE euid='no_one'), 0)", [], |r| r.get(0))
            .unwrap();
        assert_eq!(none, 0);
    }

    #[test]
    fn backfill_flags_mark_covered_rows_only() {
        let conn = in_memory_conn();
        upsert_replies(&conn, &[reply_row(1)]).unwrap();
        conn.execute("UPDATE replies SET fetched_at = 1000 WHERE pid = 1", []).unwrap();
        // 用户在 2000 完成过一次成功分析（游标已回填 = 1000）
        insert_cached_analysis(&conn, "u1", 2000, 1000);

        // 缓存生成后新抓的数据（fetched_at = 3000，模拟重抓/新抓）
        upsert_replies(&conn, &[reply_row(2)]).unwrap();
        conn.execute("UPDATE replies SET fetched_at = 3000 WHERE pid = 2", []).unwrap();

        backfill_ai_analyzed_flags(&conn).unwrap();

        // 只在分析完成时刻之前抓取的行被标记；之后抓的行保持未标记
        let analyzed: i64 = conn
            .query_row("SELECT COUNT(*) FROM replies WHERE ai_analyzed = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(analyzed, 1);
        let unanalyzed = query_unanalyzed_replies(&conn, "u1").unwrap();
        assert_eq!(unanalyzed.len(), 1);
        assert_eq!(unanalyzed[0].pid, 2);
    }

    #[test]
    fn backfill_flags_cover_refetched_rows_via_create_time() {
        let conn = in_memory_conn();
        // 分析完成时刻：晚于 pid=1 的内容发表时间（1700000001）
        let analyzed_at = 1700000100i64;
        upsert_replies(&conn, &[reply_row(1)]).unwrap();
        insert_cached_analysis(&conn, "u1", analyzed_at, 0);

        // 缓存生成后全量重抓：所有行的 fetched_at 都被刷新（晚于分析完成时刻）
        upsert_replies(&conn, &[reply_row(1), reply_row(2)]).unwrap();
        conn.execute("UPDATE replies SET fetched_at = 9999999999", []).unwrap();
        // pid=2 是缓存后新发表的内容（内容时间晚于分析完成时刻）
        conn.execute("UPDATE replies SET create_time = 1700000200 WHERE pid = 2", []).unwrap();

        backfill_ai_analyzed_flags(&conn).unwrap();

        // 老内容（pid=1）凭 create_time 判据被标记；新内容（pid=2）保持未标记
        let analyzed: i64 = conn
            .query_row("SELECT COUNT(*) FROM replies WHERE ai_analyzed = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(analyzed, 1);
        let un = query_unanalyzed_replies(&conn, "u1").unwrap();
        assert_eq!(un.len(), 1);
        assert_eq!(un[0].pid, 2);
    }

    #[test]
    fn existing_reply_pids_filters_known() {
        let conn = in_memory_conn();
        upsert_replies(&conn, &[reply_row(1), reply_row(2), reply_row(3)]).unwrap();

        // 空输入
        let empty = existing_reply_pids(&conn, &[]).unwrap();
        assert!(empty.is_empty());

        // 部分命中
        let known = existing_reply_pids(&conn, &[2, 3, 99]).unwrap();
        assert_eq!(known.len(), 2);
        assert!(known.contains(&2));
        assert!(known.contains(&3));
        assert!(!known.contains(&99));
    }

    #[test]
    fn existing_post_tids_filters_known() {
        let conn = in_memory_conn();
        let post = |tid: i64| crate::posts::PostRow {
            tid,
            euid: "u1".into(),
            username: "甲".into(),
            title: "标题".into(),
            summary: "摘要".into(),
            create_time: 1700000000,
            lastpost_time: 0,
            replies: 0,
            visits: 0,
            lights: 0,
            recommend_num: 0,
            forum_name: String::new(),
            topic_name: String::new(),
            topic_id: 0,
            total_pics: 0,
            has_video: false,
            share_num: 0,
            format_time: None,
        };
        upsert_posts(&conn, &[post(1), post(2)]).unwrap();
        let known = existing_post_tids(&conn, &[2, 3]).unwrap();
        assert_eq!(known.len(), 1);
        assert!(known.contains(&2));
        assert!(!known.contains(&3));
    }

    #[test]
    fn upsert_keeps_ai_analyzed_flag_on_refetch() {
        let conn = in_memory_conn();
        upsert_replies(&conn, &[reply_row(1)]).unwrap();
        // 模拟该行已被分析
        conn.execute("UPDATE replies SET ai_analyzed = 1 WHERE pid = 1", []).unwrap();

        // 重抓同一行（模拟 Web 上再次「获取数据」）
        upsert_replies(&conn, &[reply_row(1)]).unwrap();

        // INSERT OR REPLACE 会删除+重插导致标记清零；ON CONFLICT DO UPDATE 必须保留标记
        let flag: i64 = conn
            .query_row("SELECT ai_analyzed FROM replies WHERE pid=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(flag, 1, "重抓不得清空 ai_analyzed 标记");

        // 新插入的行（新 pid）保持未分析
        upsert_replies(&conn, &[reply_row(9)]).unwrap();
        assert_eq!(count_unanalyzed_replies(&conn, "u1").unwrap(), 1);
    }

    #[test]
    fn upsert_keeps_ai_analyzed_flag_on_post_refetch() {
        let conn = in_memory_conn();
        let post = |tid: i64| crate::posts::PostRow {
            tid,
            euid: "u1".into(),
            username: "甲".into(),
            title: "标题".into(),
            summary: "摘要".into(),
            create_time: 1700000000,
            lastpost_time: 0,
            replies: 0,
            visits: 0,
            lights: 0,
            recommend_num: 0,
            forum_name: String::new(),
            topic_name: String::new(),
            topic_id: 0,
            total_pics: 0,
            has_video: false,
            share_num: 0,
            format_time: None,
        };
        upsert_posts(&conn, &[post(1)]).unwrap();
        conn.execute("UPDATE posts SET ai_analyzed = 1 WHERE tid = 1", []).unwrap();

        upsert_posts(&conn, &[post(1)]).unwrap();

        let flag: i64 = conn
            .query_row("SELECT ai_analyzed FROM posts WHERE tid=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(flag, 1, "重抓发帖不得清空 ai_analyzed 标记");
    }

    #[test]
    fn backfill_flags_skip_users_without_successful_analysis() {
        let conn = in_memory_conn();
        upsert_replies(&conn, &[reply_row(1)]).unwrap();
        conn.execute("UPDATE replies SET fetched_at = 1000", []).unwrap();
        // 有失败批次记录 → analyzed_until 保持 0（从未成功分析）→ 行不标记
        insert_cached_analysis(&conn, "u1", 2000, 0);
        save_batch_error(&conn, "u1", "reply", 0, "boom", None).unwrap();

        backfill_ai_analyzed_flags(&conn).unwrap();

        assert_eq!(count_unanalyzed_replies(&conn, "u1").unwrap(), 1);
    }

    #[test]
    fn unanalyzed_query_and_mark() {
        let conn = in_memory_conn();
        upsert_replies(&conn, &[reply_row(1), reply_row(2), reply_row(3)]).unwrap();
        assert_eq!(count_unanalyzed_replies(&conn, "u1").unwrap(), 3);

        // 模拟其中一行已被标记
        conn.execute("UPDATE replies SET ai_analyzed = 1 WHERE pid = 1", []).unwrap();
        assert_eq!(count_unanalyzed_replies(&conn, "u1").unwrap(), 2);

        let rows = query_unanalyzed_replies(&conn, "u1").unwrap();
        assert_eq!(rows.len(), 2);

        let marked = mark_replies_analyzed(&conn, "u1").unwrap();
        assert_eq!(marked, 2);
        assert_eq!(count_unanalyzed_replies(&conn, "u1").unwrap(), 0);
        // 重复标记幂等
        assert_eq!(mark_replies_analyzed(&conn, "u1").unwrap(), 0);
    }

    #[test]
    fn mark_posts_analyzed_works() {
        let conn = in_memory_conn();
        // posts 需要有可插入的行；用最小字段插入
        let post = crate::posts::PostRow {
            tid: 999,
            euid: "u1".into(),
            username: "甲".into(),
            title: "标题".into(),
            summary: "摘要".into(),
            create_time: 1700000000,
            lastpost_time: 0,
            replies: 0,
            visits: 0,
            lights: 0,
            recommend_num: 0,
            forum_name: String::new(),
            topic_name: String::new(),
            topic_id: 0,
            total_pics: 0,
            has_video: false,
            share_num: 0,
            format_time: None,
        };
        upsert_posts(&conn, &[post]).unwrap();
        assert_eq!(count_unanalyzed_posts(&conn, "u1").unwrap(), 1);
        assert_eq!(mark_posts_analyzed(&conn, "u1").unwrap(), 1);
        assert_eq!(count_unanalyzed_posts(&conn, "u1").unwrap(), 0);
    }

    #[test]
    fn max_fetched_at_reflects_latest_fetch() {
        let conn = in_memory_conn();
        assert_eq!(max_replies_fetched_at(&conn, "u1").unwrap(), 0);

        upsert_replies(&conn, &[reply_row(1)]).unwrap();
        conn.execute("UPDATE replies SET fetched_at = 1000", []).unwrap();
        upsert_replies(&conn, &[reply_row(2)]).unwrap();
        conn.execute("UPDATE replies SET fetched_at = 2000 WHERE pid = 2", []).unwrap();

        assert_eq!(max_replies_fetched_at(&conn, "u1").unwrap(), 2000);
        assert_eq!(max_posts_fetched_at(&conn, "u1").unwrap(), 0);
    }

    #[test]
    fn save_analysis_updates_cursor() {
        let conn = in_memory_conn();
        save_ai_analysis(&conn, "u1", r#"{"label":"正常用户"}"#, 5000).unwrap();
        save_ai_analysis(&conn, "u1", r#"{"label":"更新"}"#, 8000).unwrap();
        let cursor: i64 = conn
            .query_row("SELECT analyzed_until FROM ai_analysis WHERE euid='u1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cursor, 8000);

        save_ai_post_analysis(&conn, "u1", r#"{"posts":[]}"#, 9000).unwrap();
        let cursor: i64 = conn
            .query_row("SELECT analyzed_until FROM ai_post_analysis WHERE euid='u1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cursor, 9000);
    }
}