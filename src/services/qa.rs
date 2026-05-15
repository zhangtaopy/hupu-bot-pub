use crate::db;
use crate::deepseek::{UserOverview, AgentTrace};
use std::collections::HashSet;

struct UserContext {
    username: String,
    reply_count: i64,
    post_count: i64,
    topic_vec: Vec<(String, usize)>,
    user_ctx_text: String,
    ai_reply_summary: Option<String>,
    ai_post_summary: Option<String>,
    ai_reply_personal_info: Option<String>,
}

// ── SSE event helpers ──

fn sse_round_event(trace: &AgentTrace) -> String {
    serde_json::to_string(&serde_json::json!({
        "type": "round",
        "round": trace.round,
        "action": trace.action,
        "keywords": trace.keywords,
        "search_tables": trace.search_tables,
        "reply_count": trace.reply_count,
        "post_count": trace.post_count,
        "reasoning": trace.reasoning,
        "summary_html": trace.format_md(),
    })).unwrap_or_else(|e| {
        eprintln!("[qa] sse_round_event serialize error: {e}");
        String::new()
    })
}

fn sse_answer_event(answer: &str, username: &str, detail: &str) -> String {
    serde_json::to_string(&serde_json::json!({
        "type": "answer",
        "answer": answer,
        "username": username,
        "prompt_detail": detail,
    })).unwrap_or_else(|e| {
        eprintln!("[qa] sse_answer_event serialize error: {e}");
        String::new()
    })
}

// ── Public API ──

/// Streaming: sends NDJSON events through the channel as each round completes.
pub async fn run_qa_streaming(
    db_path: &std::path::Path,
    http_client: &reqwest::Client,
    api_key: &str,
    euid: &str,
    question: &str,
    history: &[crate::server::types::HistoryEntry],
    event_tx: &tokio::sync::mpsc::Sender<String>,
) -> anyhow::Result<()> {
    let ctx = {
        let conn = db::open_db(db_path)?;
        load_user_context(&conn, euid)?
    };
    let history_ctx = crate::deepseek::format_history(history);

    let (answer, traces) = agent_loop(
        db_path, http_client, api_key, question, euid, &ctx, &history_ctx, Some(event_tx),
    ).await?;

    let detail = build_prompt_detail(&traces, &ctx.user_ctx_text);
    let _ = event_tx.send(sse_answer_event(&answer, &ctx.username, &detail)).await;

    Ok(())
}

// ── Core agent loop ──

async fn agent_loop(
    db_path: &std::path::Path,
    http_client: &reqwest::Client,
    api_key: &str,
    question: &str,
    euid: &str,
    ctx: &UserContext,
    history_ctx: &str,
    event_tx: Option<&tokio::sync::mpsc::Sender<String>>,
) -> anyhow::Result<(String, Vec<AgentTrace>)> {
    let mut all_replies: Vec<crate::replies::ReplyRow> = Vec::new();
    let mut all_posts: Vec<crate::posts::PostRow> = Vec::new();
    let mut seen_reply_pids: HashSet<i64> = HashSet::new();
    let mut seen_post_tids: HashSet<i64> = HashSet::new();
    let mut traces: Vec<AgentTrace> = Vec::new();
    let mut previous_rounds_text = String::new();
    let mut final_answer = String::new();
    let conn = db::open_db(db_path)?;

    let max_rounds = 5;
    for round in 1..=max_rounds {
        let action = crate::deepseek::agent_decide(
            http_client, api_key, question, &ctx.user_ctx_text, history_ctx,
            &previous_rounds_text, round,
        ).await?;

        if action.action == "final_answer" {
            final_answer = action.answer;
            let trace = AgentTrace {
                round,
                action: "给出最终回答".into(),
                reasoning: String::new(),
                keywords: vec![],
                search_tables: vec![],
                reply_count: all_replies.len(),
                post_count: all_posts.len(),
            };
            emit(event_tx, &sse_round_event(&trace)).await;
            traces.push(trace);
            break;
        }

        let search_replies = action.search_tables.is_empty() || action.search_tables.iter().any(|t| t == "replies");
        let search_posts = action.search_tables.is_empty() || action.search_tables.iter().any(|t| t == "posts");

        let round_replies = if search_replies && !action.keywords.is_empty() {
            db::search_replies(&conn, euid, &action.keywords, &action.topic_filter, &action.sort_by, action.max_results)?
        } else {
            Vec::new()
        };

        let round_posts = if search_posts && !action.keywords.is_empty() {
            db::search_posts(&conn, euid, &action.keywords, &action.topic_filter, &action.sort_by, action.max_results)?
        } else {
            Vec::new()
        };

        let new_reply_count = round_replies.iter().filter(|r| !seen_reply_pids.contains(&r.pid)).count();
        let new_post_count = round_posts.iter().filter(|p| !seen_post_tids.contains(&p.tid)).count();

        for r in &round_replies {
            if seen_reply_pids.insert(r.pid) {
                all_replies.push(r.clone());
            }
        }
        for p in &round_posts {
            if seen_post_tids.insert(p.tid) {
                all_posts.push(p.clone());
            }
        }

        let round_summary = crate::deepseek::format_search_results_summary(&round_replies, &round_posts);
        previous_rounds_text.push_str(&format!(
            "=== 第{}轮搜索 (关键词: {}) ===\n{}\n\n",
            round,
            action.keywords.join("、"),
            round_summary,
        ));

        let trace = AgentTrace {
            round,
            action: "搜索".into(),
            reasoning: action.reasoning,
            keywords: action.keywords,
            search_tables: action.search_tables,
            reply_count: new_reply_count,
            post_count: new_post_count,
        };
        emit(event_tx, &sse_round_event(&trace)).await;
        traces.push(trace);
    }

    if final_answer.is_empty() {
        let overview = build_overview(
            &conn, euid, ctx.reply_count, ctx.post_count,
            ctx.topic_vec.clone(), ctx.ai_reply_summary.clone(),
            ctx.ai_post_summary.clone(), ctx.ai_reply_personal_info.clone(),
        )?;
        final_answer = crate::deepseek::generate_answer(
            http_client, api_key, question, &ctx.username, &overview,
            &all_replies, &all_posts, history_ctx,
        ).await?;
        let trace = AgentTrace {
            round: max_rounds + 1,
            action: "达到最大轮数，综合所有结果给出最终回答".into(),
            reasoning: String::new(),
            keywords: vec![],
            search_tables: vec![],
            reply_count: all_replies.len(),
            post_count: all_posts.len(),
        };
        emit(event_tx, &sse_round_event(&trace)).await;
        traces.push(trace);
    }

    Ok((final_answer, traces))
}

async fn emit(tx: Option<&tokio::sync::mpsc::Sender<String>>, data: &str) {
    if let Some(tx) = tx {
        if tx.send(data.to_string()).await.is_err() {
            eprintln!("[qa] emit error: channel closed");
        }
    }
}

// ── Load user context ──

fn load_user_context(conn: &rusqlite::Connection, euid: &str) -> anyhow::Result<UserContext> {
    let reply_count = db::count_replies(conn, Some(euid))?;
    let post_count = db::count_posts(conn, Some(euid))?;
    if reply_count == 0 && post_count == 0 {
        return Err(anyhow::anyhow!("该用户没有回帖和发帖数据，请先获取数据"));
    }

    let username: String = db::get_username(conn, euid)?
        .unwrap_or_else(|| "未知用户".to_string());

    let ai_reply_summary = load_ai_reply_summary(conn, euid);
    let ai_reply_personal_info = load_ai_personal_info(conn, euid);
    let ai_post_summary = load_ai_post_summary(conn, euid);

    let topic_dist = db::query_topic_distribution(conn, euid).unwrap_or_default();
    let mut topic_vec: Vec<(String, usize)> = topic_dist.into_iter().collect();
    topic_vec.sort_by(|a, b| b.1.cmp(&a.1));
    topic_vec.truncate(10);

    let user_ctx_text = build_user_ctx_text(
        &username, reply_count, post_count, &topic_vec,
        &ai_reply_personal_info, &ai_reply_summary,
    );

    Ok(UserContext {
        username,
        reply_count,
        post_count,
        topic_vec,
        user_ctx_text,
        ai_reply_summary,
        ai_post_summary,
        ai_reply_personal_info,
    })
}

fn load_ai_reply_summary(conn: &rusqlite::Connection, euid: &str) -> Option<String> {
    db::get_ai_analysis(conn, euid)
        .unwrap_or_else(|e| {
            eprintln!("[qa] load_ai_reply_summary db error: {e}");
            None
        })
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        .and_then(|v| v.get("summary").and_then(|s| s.as_str().map(String::from)))
}

fn load_ai_personal_info(conn: &rusqlite::Connection, euid: &str) -> Option<String> {
    db::get_ai_analysis(conn, euid)
        .unwrap_or_else(|e| {
            eprintln!("[qa] load_ai_personal_info db error: {e}");
            None
        })
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        .and_then(|v| {
            let pi = &v["personal_info"];
            if pi.is_object() { Some(format_personal_info(pi)) } else { None }
        })
}

fn load_ai_post_summary(conn: &rusqlite::Connection, euid: &str) -> Option<String> {
    db::get_ai_post_analysis(conn, euid)
        .unwrap_or_else(|e| {
            eprintln!("[qa] load_ai_post_summary db error: {e}");
            None
        })
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        .and_then(|v| v.get("summary").and_then(|s| s.as_str().map(String::from)))
}

fn build_user_ctx_text(
    username: &str,
    reply_count: i64,
    post_count: i64,
    topic_vec: &[(String, usize)],
    ai_reply_personal_info: &Option<String>,
    ai_reply_summary: &Option<String>,
) -> String {
    let mut text = format!(
        "用户名: {}\n总回帖: {}条\n总发帖: {}条\n",
        username, reply_count, post_count
    );
    if !topic_vec.is_empty() {
        let topics: Vec<String> = topic_vec.iter()
            .map(|(t, c)| format!("{}({}条)", t, c))
            .collect();
        text.push_str(&format!("活跃板块: {}\n", topics.join("、")));
    }
    if let Some(ref info) = ai_reply_personal_info {
        text.push_str(&format!("\n已推断的用户信息:\n{}\n", info));
    }
    if let Some(ref summary) = ai_reply_summary {
        text.push_str(&format!("\nAI画像摘要: {}\n", summary));
    }
    text
}

fn build_overview(
    conn: &rusqlite::Connection,
    euid: &str,
    reply_count: i64,
    post_count: i64,
    topic_vec: Vec<(String, usize)>,
    ai_reply_summary: Option<String>,
    ai_post_summary: Option<String>,
    ai_reply_personal_info: Option<String>,
) -> anyhow::Result<UserOverview> {
    let time_vec: Vec<(String, usize)> = db::query_time_distribution(conn, euid)
        .unwrap_or_default()
        .into_iter()
        .collect();

    Ok(UserOverview {
        total_replies: reply_count,
        total_posts: post_count,
        topic_distribution: topic_vec,
        reply_time_distribution: time_vec,
        activity_period: compute_activity_period(conn, euid),
        ai_reply_analysis_summary: ai_reply_summary,
        ai_post_analysis_summary: ai_post_summary,
        ai_reply_personal_info,
    })
}

fn compute_activity_period(conn: &rusqlite::Connection, euid: &str) -> Option<String> {
    let reply_range = conn.query_row(
        "SELECT MIN(create_time), MAX(create_time) FROM replies WHERE euid = ?",
        rusqlite::params![euid],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    ).ok();
    let post_range = conn.query_row(
        "SELECT MIN(create_time), MAX(create_time) FROM posts WHERE euid = ?",
        rusqlite::params![euid],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    ).ok();

    let first = reply_range.map(|(f, _)| f).into_iter()
        .chain(post_range.map(|(f, _)| f))
        .min();
    let last = reply_range.map(|(_, l)| l).into_iter()
        .chain(post_range.map(|(_, l)| l))
        .max();

    match (first, last) {
        (Some(f), Some(l)) => {
            let fmt = |ts: i64| {
                chrono::DateTime::from_timestamp(ts, 0)
                    .map(|dt| dt.format("%Y-%m").to_string())
                    .unwrap_or_default()
            };
            Some(format!("{} ~ {}", fmt(f), fmt(l)))
        }
        _ => None,
    }
}

fn build_prompt_detail(traces: &[AgentTrace], user_ctx_text: &str) -> String {
    let trace_text: String = traces.iter()
        .map(|t| t.format_md())
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "=== 用户上下文 ===\n{}\n\n=== Agent决策过程 ===\n{}",
        user_ctx_text, trace_text
    )
}

fn format_personal_info(pi: &serde_json::Value) -> String {
    let fields: &[(&str, &str)] = &[
        ("年龄段", "age_range"),
        ("性别", "gender"),
        ("身高体重", "height_weight"),
        ("感情状况", "relationship"),
        ("籍贯", "hometown"),
        ("现居城市", "current_city"),
        ("教育背景", "education"),
        ("留学经历", "study_abroad"),
        ("职业", "profession"),
        ("收入水平", "income_hint"),
        ("车辆", "car"),
        ("房产", "housing"),
        ("性格特征", "personality_traits"),
        ("政治倾向", "political_stance"),
    ];

    let mut lines = Vec::new();
    for (label, key) in fields {
        if let Some(v) = pi.get(key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                lines.push(format!("  {}: {}", label, v));
            }
        }
    }

    for (label, key) in &[("主队", "sports_teams"), ("爱好", "hobbies"), ("游戏", "games")] {
        if let Some(arr) = pi.get(key).and_then(|v| v.as_array()) {
            let items: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
            if !items.is_empty() {
                lines.push(format!("  {}: {}", label, items.join("、")));
            }
        }
    }

    if lines.is_empty() {
        return String::new();
    }
    lines.join("\n")
}
