use crate::db;
use crate::deepseek::UserOverview;

pub struct QaResult {
    pub answer: String,
    pub username: String,
    pub prompt_detail: String,
}

/// 4-step Q&A flow:
///   1. Quick DB query → basic user stats + existing AI analysis
///   2. AI intent recognition (with user context) → query plan
///   3. Execute keyword search on DB + build full overview
///   4. AI generates answer from overview and query results
pub async fn run_qa(
    db_path: &std::path::Path,
    http_client: &reqwest::Client,
    api_key: &str,
    euid: &str,
    question: &str,
) -> anyhow::Result<QaResult> {
    // Step 1: Quick DB query for basic user context
    let conn = db::open_db(db_path)?;

    let reply_count = db::count_replies(&conn, Some(euid))?;
    let post_count = db::count_posts(&conn, Some(euid))?;
    if reply_count == 0 && post_count == 0 {
        return Err(anyhow::anyhow!("该用户没有回帖和发帖数据，请先获取数据"));
    }

    // Get username
    let username = {
        let all_replies = db::query_replies(&conn, Some(euid), 1, 0)?;
        let all_posts = db::query_posts(&conn, Some(euid), 1, 0)?;
        all_replies
            .first()
            .map(|r| r.username.clone())
            .or_else(|| all_posts.first().map(|p| p.username.clone()))
            .unwrap_or_else(|| "未知用户".to_string())
    };

    // Load existing AI analysis results for context
    let ai_reply_summary = db::get_ai_analysis(&conn, euid)
        .unwrap_or(None)
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        .and_then(|v| v.get("summary").and_then(|s| s.as_str().map(String::from)));

    let ai_reply_personal_info = db::get_ai_analysis(&conn, euid)
        .unwrap_or(None)
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        .and_then(|v| {
            let pi = &v["personal_info"];
            if pi.is_object() {
                Some(format_personal_info(pi))
            } else {
                None
            }
        });

    let ai_post_summary = db::get_ai_post_analysis(&conn, euid)
        .unwrap_or(None)
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        .and_then(|v| v.get("summary").and_then(|s| s.as_str().map(String::from)));

    // Topic distribution (top topics)
    let topic_dist = db::query_topic_distribution(&conn, euid)
        .unwrap_or_default();
    let mut topic_vec: Vec<(String, usize)> = topic_dist.into_iter().collect();
    topic_vec.sort_by(|a, b| b.1.cmp(&a.1));
    topic_vec.truncate(10);

    // Build user context string for intent recognition
    let mut user_ctx = format!(
        "用户名: {}\n总回帖: {}条\n总发帖: {}条\n",
        username, reply_count, post_count
    );
    if !topic_vec.is_empty() {
        let topics: Vec<String> = topic_vec.iter()
            .map(|(t, c)| format!("{}({}条)", t, c))
            .collect();
        user_ctx.push_str(&format!("活跃板块: {}\n", topics.join("、")));
    }
    if let Some(ref info) = ai_reply_personal_info {
        user_ctx.push_str(&format!("\n已推断的用户信息:\n{}\n", info));
    }
    if let Some(ref summary) = ai_reply_summary {
        user_ctx.push_str(&format!("\nAI画像摘要: {}\n", summary));
    }

    // Step 2: AI intent recognition with user context
    let plan = crate::deepseek::recognize_intent(http_client, api_key, question, &user_ctx).await?;

    // Step 3: Keyword search based on intent
    let search_replies_table = plan.search_tables.iter().any(|t| t == "replies");
    let search_posts_table = plan.search_tables.iter().any(|t| t == "posts");
    let search_replies = plan.search_tables.is_empty() || search_replies_table;
    let search_posts = plan.search_tables.is_empty() || search_posts_table;

    let replies = if search_replies && !plan.search_keywords.is_empty() {
        db::search_replies(&conn, euid, &plan.search_keywords, &plan.topic_filter, &plan.sort_by, plan.max_results)?
    } else {
        Vec::new()
    };

    let posts = if search_posts && !plan.search_keywords.is_empty() {
        db::search_posts(&conn, euid, &plan.search_keywords, &plan.topic_filter, &plan.sort_by, plan.max_results)?
    } else {
        Vec::new()
    };

    // Time distribution and activity period
    let time_dist = db::query_time_distribution(&conn, euid)
        .unwrap_or_default();
    let time_vec: Vec<(String, usize)> = time_dist.into_iter().collect();

    let activity_period = {
        let first_reply = conn.query_row(
            "SELECT create_time FROM replies WHERE euid = ? ORDER BY create_time ASC LIMIT 1",
            rusqlite::params![euid],
            |row| row.get::<_, i64>(0),
        ).ok();
        let last_reply = conn.query_row(
            "SELECT create_time FROM replies WHERE euid = ? ORDER BY create_time DESC LIMIT 1",
            rusqlite::params![euid],
            |row| row.get::<_, i64>(0),
        ).ok();
        let first_post = conn.query_row(
            "SELECT create_time FROM posts WHERE euid = ? ORDER BY create_time ASC LIMIT 1",
            rusqlite::params![euid],
            |row| row.get::<_, i64>(0),
        ).ok();
        let last_post = conn.query_row(
            "SELECT create_time FROM posts WHERE euid = ? ORDER BY create_time DESC LIMIT 1",
            rusqlite::params![euid],
            |row| row.get::<_, i64>(0),
        ).ok();

        let first = first_reply.into_iter()
            .chain(first_post.into_iter())
            .min();
        let last = last_reply.into_iter()
            .chain(last_post.into_iter())
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
    };

    // Build full overview
    let overview = UserOverview {
        total_replies: reply_count,
        total_posts: post_count,
        topic_distribution: topic_vec,
        reply_time_distribution: time_vec,
        activity_period,
        ai_reply_analysis_summary: ai_reply_summary,
        ai_post_analysis_summary: ai_post_summary,
        ai_reply_personal_info,
    };

    // Step 4: AI generates the answer
    let answer = crate::deepseek::generate_answer(
        http_client, api_key, question, &username, &overview, &replies, &posts,
    ).await?;

    // Build prompt_detail for frontend display
    let overview_text = overview.format();
    let search_summary = format!(
        "搜索关键词: {}\n搜索表: {}\n板块过滤: {}\n排序: {}\n最多结果: {}",
        plan.search_keywords.join("、"),
        plan.search_tables.join("、"),
        if plan.topic_filter.is_empty() { "无".into() } else { plan.topic_filter.join("、") },
        &plan.sort_by,
        plan.max_results,
    );
    let detail = format!(
        "=== 用户上下文（用于意图识别）===\n{}\n\n=== 查询计划 ===\n{}\n\n=== 用户概览 ===\n{}\n\n=== 搜索结果（共{}条回帖 + {}条发帖）===\n{}",
        user_ctx, search_summary, overview_text, replies.len(), posts.len(),
        crate::deepseek::format_query_results(&replies, &posts)
    );

    Ok(QaResult { answer, username, prompt_detail: detail })
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
