use std::sync::Arc;

use crate::client::HupuClient;
use crate::deepseek::AiProvider;
use crate::server::types::{AppState, ProgressState};
use crate::topic;

/// Background task: fetch posts + hot replies for a given topic_id, date-based with dedup.
///
/// - `days`: fetch posts from the last N days (e.g. 1 = today only, 7 = past week)
/// - `replies_per_post`: max hot replies to fetch per post
/// - Skips dates already covered in the DB for this topic_id
pub async fn run_fetch_monitor_background(
    state: Arc<AppState>,
    topic_id: String,
    days: u32,
    replies_per_post: usize,
    cookie_override: Option<String>,
) {
    let key = format!("monitor_fetch:{}", topic_id);

    let set_progress =
        |phase: &str, current: usize, total: usize, done: bool, error: Option<String>| {
            if let Ok(mut p) = state.progress.lock() {
                p.insert(
                    key.clone(),
                    ProgressState {
                        phase: phase.into(),
                        current,
                        total,
                        done,
                        error,
                    },
                );
            }
        };

    set_progress("准备中", 0, 0, false, None);

    // Resolve cookie
    let cookie_str = match cookie_override
        .filter(|c| !c.is_empty())
        .or_else(|| {
            crate::config::try_get()
                .and_then(|c| if !c.cookie.is_empty() { Some(c.cookie) } else { None })
        }) {
        Some(c) => c,
        None => {
            set_progress("error", 0, 0, true, Some("请先配置 Cookie".into()));
            return;
        }
    };

    let client = match HupuClient::new(&cookie_str) {
        Ok(c) => c,
        Err(e) => {
            set_progress("error", 0, 0, true, Some(format!("创建客户端失败: {}", e)));
            return;
        }
    };

    let conn = match crate::db::open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => {
            set_progress("error", 0, 0, true, Some(format!("打开数据库失败: {}", e)));
            return;
        }
    };

    // Build set of already-known TIDs for this topic (skip re-fetching replies)
    let known_tids: std::collections::HashSet<i64> =
        match crate::db::get_monitor_known_tids(&conn, &topic_id) {
            Ok(tids) => tids.into_iter().collect(),
            Err(_) => std::collections::HashSet::new(),
        };

    // ═══════════════════════════════════════════════════════
    // Phase 1: Collect all new posts from topic listing pages
    // (fast — just scraping HTML, shouldn't trigger anti-crawl)
    // ═══════════════════════════════════════════════════════
    let mut all_new_posts: Vec<topic::Post> = Vec::new();
    let mut skipped_count = 0usize;
    let mut consecutive_empty = 0u32;
    let max_pages = 20;
    let max_empty_pages = 3;

    for page in 1..=max_pages {
        set_progress(
            &format!("获取帖子列表 第{}页 (已收集 {} 个新帖)", page, all_new_posts.len()),
            all_new_posts.len(),
            0, // unknown total → indeterminate bar
            false,
            None,
        );

        let posts = match topic::fetch_topic_posts(&client, &topic_id, page, 50).await {
            Ok(p) => p,
            Err(e) => {
                set_progress(
                    "error",
                    all_new_posts.len(),
                    all_new_posts.len(),
                    true,
                    Some(format!("获取帖子列表失败: {}", e)),
                );
                return;
            }
        };

        if posts.is_empty() {
            break;
        }

        let mut page_has_new = false;
        for post in posts {
            let tid_num: i64 = post.tid.parse().unwrap_or(0);
            if known_tids.contains(&tid_num) {
                skipped_count += 1;
            } else {
                all_new_posts.push(post);
                page_has_new = true;
            }
        }

        if !page_has_new {
            consecutive_empty += 1;
            if consecutive_empty >= max_empty_pages {
                set_progress(
                    &format!("连续{}页无新帖，停止翻页", consecutive_empty),
                    all_new_posts.len(), all_new_posts.len(), false, None,
                );
                break;
            }
        } else {
            consecutive_empty = 0;
        }
    }

    let total_new = all_new_posts.len();

    if total_new == 0 {
        let mut summary = "完成: 0 帖子, 0 热评".to_string();
        if skipped_count > 0 {
            summary.push_str(&format!(" | 跳过 {} 条已有帖子", skipped_count));
        }
        set_progress(&summary, 0, 0, true, None);
        return;
    }

    // ═══════════════════════════════════════════════════════
    // Phase 2: Fetch details in batches with a pause between
    // batches to avoid triggering anti-crawl.
    // 10 posts per batch, then 2s gap.
    // ═══════════════════════════════════════════════════════
    let mut total_posts = 0usize;
    let mut total_replies = 0usize;
    let batch_size = 10usize;
    let batch_delay = std::time::Duration::from_secs(2);

    for (i, post) in all_new_posts.iter().enumerate() {
        set_progress(
            &format!("获取详情 {}/{}: {}", i + 1, total_new, truncate_str(&post.title, 30)),
            i + 1,
            total_new,
            false,
            None,
        );

        let tid_num: i64 = post.tid.parse().unwrap_or(0);

        // Fetch detail page for accurate create_time + hot replies
        let (real_ts, real_replies, real_lights, real_author) =
            match topic::fetch_post_combined(&client, &post.tid, replies_per_post).await {
                Ok((detail, replies)) => {
                    let ts = detail.create_time.as_deref()
                        .and_then(|t| parse_iso_time(t))
                        .unwrap_or_else(|| chrono::Utc::now().timestamp());

                    // Check if post is within date range
                    let post_age_days = (chrono::Utc::now().timestamp() - ts) / 86400;
                    if post_age_days > days as i64 {
                        continue; // too old, skip — delay handled at end of loop
                    }

                    // Store replies
                    if !replies.is_empty() {
                        if let Err(_) = crate::db::upsert_monitor_replies(
                            &conn, &topic_id, tid_num, &replies,
                        ) {} else {
                            total_replies += replies.len();
                        }
                    }

                    (ts, detail.reply_count.unwrap_or(0), detail.light_count.unwrap_or(0), detail.author)
                }
                Err(_) => {
                    // Detail unavailable — use listing data (less accurate but better than nothing)
                    let ts = post.create_time.as_deref()
                        .and_then(|t| crate::db::parse_post_time_to_ts(t))
                        .unwrap_or_else(|| chrono::Utc::now().timestamp());
                    (ts, post.reply_count.unwrap_or(0), post.light_count.unwrap_or(0),
                     post.author.clone().unwrap_or_default())
                }
            };

        // Insert post with accurate metadata
        let post_data = vec![topic::Post {
            tid: post.tid.clone(),
            title: post.title.clone(),
            author: Some(real_author.clone()),
            reply_count: Some(real_replies),
            light_count: Some(real_lights),
            create_time: Some(chrono::DateTime::from_timestamp(real_ts, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default()),
        }];
        if let Err(e) = crate::db::upsert_monitor_posts(&conn, &topic_id, &post_data) {
            eprintln!("[monitor] upsert failed tid={}: {}", post.tid, e);
        } else {
            total_posts += 1;
        }

        // Pause after every batch (10 posts) — skip after the last post
        let n = i + 1;
        if n % batch_size == 0 && n < total_new {
            set_progress(
                &format!("已获取 {}/{}，暂停 {} 秒...", n, total_new, batch_delay.as_secs()),
                n, total_new, false, None,
            );
            tokio::time::sleep(batch_delay).await;
        }
    }

    let mut summary = format!("完成: {} 帖子, {} 热评", total_posts, total_replies);
    if skipped_count > 0 {
        summary.push_str(&format!(" | 跳过 {} 条已有帖子", skipped_count));
    }

    set_progress(&summary, total_posts, total_posts, true, None);
}

/// (removed — metadata now fetched inline during initial fetch)
#[allow(dead_code)]
async fn update_post_metadata(
    db_path: &std::path::Path,
    client: &HupuClient,
    topic_id: &str,
) -> usize {
    // Step 1: collect TIDs
    let posts_to_fix: Vec<i64> = match crate::db::open_db(db_path) {
        Ok(conn) => {
            match conn.prepare(
                "SELECT tid FROM monitor_posts WHERE topic_id = ?1 AND reply_count = 0 LIMIT 30"
            ) {
                Ok(mut stmt) => {
                    match stmt.query_map(rusqlite::params![topic_id], |row| row.get(0)) {
                        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
                        Err(_) => Vec::new(),
                    }
                }
                Err(_) => Vec::new(),
            }
        }
        Err(_) => Vec::new(),
    };

    if posts_to_fix.is_empty() {
        return 0;
    }

    // Step 2: fetch details and collect updates (conn dropped, only client used)
    let mut updates: Vec<(i64, i32, i32, String)> = Vec::new();
    for tid in &posts_to_fix {
        let tid_str = tid.to_string();
        match crate::topic::fetch_post_detail(client, &tid_str).await {
            Ok(detail) => {
                updates.push((
                    *tid,
                    detail.reply_count.unwrap_or(0),
                    detail.light_count.unwrap_or(0),
                    detail.author,
                ));
            }
            Err(e) => {
                eprintln!("[monitor] fetch detail failed tid={}: {}", tid, e);
            }
        }
    }

    // Step 3: apply updates
    let count = updates.len();
    if let Ok(conn) = crate::db::open_db(db_path) {
        if let Ok(tx) = conn.unchecked_transaction() {
        for (tid, replies, lights, author) in &updates {
            let _ = tx.execute(
                "UPDATE monitor_posts SET reply_count = ?1, light_count = ?2, author = ?3 WHERE tid = ?4",
                rusqlite::params![replies, lights, author, tid],
            );
        }
            let _ = tx.commit();
        }
    }

    count
}

/// Background task: AI sentiment analysis on monitor data
pub async fn run_analyze_monitor_background(
    state: Arc<AppState>,
    topic_id: String,
    user_provider: Option<AiProvider>,
) {
    let key = format!("monitor_analyze:{}", topic_id);

    let set_progress =
        |phase: &str, current: usize, total: usize, done: bool, error: Option<String>| {
            if let Ok(mut p) = state.progress.lock() {
                p.insert(
                    key.clone(),
                    ProgressState {
                        phase: phase.into(),
                        current,
                        total,
                        done,
                        error,
                    },
                );
            }
        };

    set_progress("准备 AI 分析", 0, 0, false, None);

    let provider = if let Some(p) = user_provider {
        p
    } else {
        let cfg = match crate::config::try_get() {
            Some(cfg) => cfg,
            None => {
                set_progress("error", 0, 0, true, Some("请先配置 AI API Key".into()));
                return;
            }
        };
        if cfg.api_key.is_empty() {
            set_progress("error", 0, 0, true, Some("未配置 AI API Key".into()));
            return;
        }
        AiProvider::from_user_input(&cfg.provider, &cfg.api_key)
    };

    let conn = match crate::db::open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => {
            set_progress("error", 0, 0, true, Some(format!("打开数据库失败: {}", e)));
            return;
        }
    };

    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let post_count =
        crate::db::count_monitor_posts(&conn, &topic_id, Some(&today)).unwrap_or(0);
    let reply_count =
        crate::db::count_monitor_replies(&conn, &topic_id, Some(&today)).unwrap_or(0);

    let replies =
        match crate::db::query_monitor_replies(&conn, &topic_id, Some(&today), 100, 0) {
            Ok(r) => r,
            Err(e) => {
                set_progress("error", 0, 0, true, Some(format!("查询热评失败: {}", e)));
                return;
            }
        };

    if replies.is_empty() {
        let _ = crate::db::save_monitor_snapshot(
            &conn,
            &topic_id,
            &today,
            post_count,
            reply_count,
            r#"{"positive":0,"neutral":0,"negative":0}"#,
            "[]",
            "暂无热评数据，无法进行舆情分析。",
            "{}",
        );
        set_progress("完成 (无热评)", 0, 0, true, None);
        return;
    }

    // Get top posts for context
    let posts = crate::db::query_monitor_posts(&conn, &topic_id, Some(&today), 30, 0)
        .unwrap_or_default();

    let mut context_text = String::new();

    // Post titles
    if !posts.is_empty() {
        context_text.push_str("## 今日热帖标题\n\n");
        for (i, p) in posts.iter().take(30).enumerate() {
            let title = p.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let replies = p.get("reply_count").and_then(|v| v.as_i64()).unwrap_or(0);
            let lights = p.get("light_count").and_then(|v| v.as_i64()).unwrap_or(0);
            context_text.push_str(&format!(
                "{}. [{}回复/{}亮] {}\n",
                i + 1,
                replies,
                lights,
                title
            ));
        }
    }

    // Hot replies
    context_text.push_str("\n## 热门回复 (按点赞排序)\n\n");
    for (i, r) in replies.iter().take(80).enumerate() {
        let content = r.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let lights = r.get("light_count").and_then(|v| v.as_i64()).unwrap_or(0);
        let username = r.get("username").and_then(|v| v.as_str()).unwrap_or("");
        context_text.push_str(&format!(
            "{}. [{}赞] {}: {}\n",
            i + 1,
            lights,
            username,
            truncate_str(content, 150)
        ));
    }

    let system_prompt = r##"你是一位资深互联网社区舆论分析师。请基于虎扑论坛热帖和热门回复，撰写一份舆情日报。严格返回JSON，不要markdown包裹。分析要具体、有数据、有洞察。"##;

    let user_prompt = format!(
        r##"请分析以下虎扑论坛板块今日舆情数据（{}条热帖、{}条热评），返回JSON：

## 分析要求

1. **sentiment**: 统计正面/中性/负面回复的具体条数

2. **top_keywords**: 10-15个今日高频热词（产品名、人名、事件、话题标签等）

3. **summary**: 200-300字舆情摘要，涵盖：今日讨论主线与舆论基调；最受关注的事件及倾向；值得关注的新趋势

4. **brand_analysis**: 列出今日被重点讨论的主体/品牌/组织（5-8个），每个包含：
   - brand: 名称
   - mention_count: 提及次数
   - sentiment: "正面"/"负面"/"争议"
   - key_points: 主要讨论点（2-3条）
   - representative_post: 最有代表性的帖子标题

5. **hot_topics**: 3-5个今日最热话题，每个包含：
   - topic: 话题名
   - heat: "🔥🔥🔥"/"🔥🔥"/"🔥"
   - sentiment: "正面"/"负面"/"争议"
   - summary: 讨论焦点
   - user_consensus: 主流观点

6. **opinion_camps**: 1-3组对立观点阵营，每组包含 topic / side_a / side_b / ratio

7. **notable_quotes**: 3-5条精彩评论（≤60字每条），标注情感倾向

8. **key_models**: 今日最受关注的5-8个具体产品/型号/人物/球队等，每个包含名称、热度、好评/差评占比

## 数据

{}
严格返回JSON，不要任何markdown包裹。"##,
        posts.len(), replies.len().min(80),
        context_text
    );

    set_progress("调用 AI 分析中", 1, 3, false, None);

    let ai_result = match crate::deepseek::call_llm_text_with_tokens(
        &state.http_client,
        &provider,
        system_prompt,
        &user_prompt,
        16384,  // large enough for full brand+model+topic analysis
    )
    .await
    {
        Ok((content, _usage)) => content,
        Err(e) => {
            set_progress("error", 0, 0, true, Some(format!("AI 调用失败: {}", e)));
            return;
        }
    };

    set_progress("解析 AI 结果", 2, 3, false, None);

    let preview_end = ai_result.ceil_char_boundary(ai_result.len().min(2000));
    eprintln!("[AI-RAW] {}", &ai_result[..preview_end]);
    let cleaned = clean_json_response(&ai_result);
    let parsed: serde_json::Value = match serde_json::from_str(&cleaned) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[AI-PARSE-ERR] {} | cleaned(first 300): {}", e, &cleaned[..cleaned.len().min(300)]);
            serde_json::json!({
                "sentiment": {"positive": 0, "neutral": 0, "negative": 0},
                "top_keywords": [],
                "summary": ai_result,
                "hot_topics": []
            })
        }
    };

    let sentiment_dist = parsed.get("sentiment").map(|s| s.to_string()).unwrap_or_default();
    let top_keywords = parsed.get("top_keywords").map(|k| k.to_string()).unwrap_or_else(|| "[]".to_string());

    // Build rich AI report in Markdown
    let mut md = String::new();

    // Summary
    if let Some(s) = parsed.get("summary").and_then(|v| v.as_str()) {
        md.push_str(s);
        md.push_str("\n\n");
    }

    // Brand analysis table
    if let Some(brands) = parsed.get("brand_analysis").and_then(|v| v.as_array()) {
        if !brands.is_empty() {
            md.push_str("## 品牌热议榜\n\n");
            md.push_str("| 品牌 | 热度 | 风评 | 关键讨论 |\n");
            md.push_str("|------|------|------|----------|\n");
            for b in brands {
                let brand = b.get("brand").and_then(|v| v.as_str()).unwrap_or("");
                let count = b.get("mention_count").and_then(|v| v.as_i64()).unwrap_or(0);
                let sentiment = b.get("sentiment").and_then(|v| v.as_str()).unwrap_or("");
                let points: String = b.get("key_points").and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|p| p.as_str()).collect::<Vec<_>>().join("；"))
                    .unwrap_or_default();
                let sent_icon = match sentiment {
                    "正面" => "🟢",
                    "负面" => "🔴",
                    "争议" => "🟡",
                    _ => "⚪",
                };
                md.push_str(&format!("| {} | {}次 | {} {} | {} |\n", brand, count, sent_icon, sentiment, points));
            }
            md.push_str("\n");
        }
    }

    // Key models
    if let Some(models) = parsed.get("key_models").and_then(|v| v.as_array()) {
        if !models.is_empty() {
            md.push_str("## 热门车型\n\n");
            for m in models {
                let name = m.get("name").or(m.get("model")).or(m.get("car"))
                    .and_then(|v| v.as_str()).unwrap_or("");
                let heat = m.get("heat").or(m.get("popularity"))
                    .and_then(|v| v.as_str()).unwrap_or("");
                let pos = m.as_object().and_then(|obj| find_positive_pct(obj));
                let neg = m.as_object().and_then(|obj| find_negative_pct(obj));
                let rating_str = match (pos, neg) {
                    (Some(p), Some(n)) => format!("👍{}% / 👎{}%", normalize_pct(p), normalize_pct(n)),
                    (Some(p), None) => format!("👍{}%", normalize_pct(p)),
                    (None, Some(n)) => format!("👎{}%", normalize_pct(n)),
                    (None, None) => String::new(),
                };
                if !name.is_empty() {
                    md.push_str(&format!("- **{}** — 热度: {} | {}\n", name, heat, rating_str));
                }
            }
            md.push_str("\n");
        }
    }

    // Hot topics
    if let Some(topics) = parsed.get("hot_topics").and_then(|v| v.as_array()) {
        if !topics.is_empty() {
            md.push_str("## 热点话题\n\n");
            for t in topics {
                let topic = t.get("topic").and_then(|v| v.as_str()).unwrap_or("");
                let heat = t.get("heat").and_then(|v| v.as_str()).unwrap_or("");
                let summary = t.get("summary").and_then(|v| v.as_str()).unwrap_or("");
                let consensus = t.get("user_consensus").and_then(|v| v.as_str()).unwrap_or("");
                md.push_str(&format!("### {} {}\n{}\n\n> {}\n\n", heat, topic, summary, consensus));
            }
        }
    }

    // Opinion camps
    if let Some(camps) = parsed.get("opinion_camps").and_then(|v| v.as_array()) {
        if !camps.is_empty() {
            md.push_str("## 观点交锋\n\n");
            for camp in camps {
                let topic = camp.get("topic").and_then(|v| v.as_str()).unwrap_or("");
                let side_a = camp.get("side_a").and_then(|v| v.as_str()).unwrap_or("");
                let side_b = camp.get("side_b").and_then(|v| v.as_str()).unwrap_or("");
                let ratio = camp.get("ratio").and_then(|v| v.as_str()).unwrap_or("");
                md.push_str(&format!("**{}** ({})\n- 👍 {}\n- 👎 {}\n\n", topic, ratio, side_a, side_b));
            }
        }
    }

    // Notable quotes
    if let Some(quotes) = parsed.get("notable_quotes").and_then(|v| v.as_array()) {
        if !quotes.is_empty() {
            md.push_str("## 精彩评论\n\n");
            for q in quotes {
                // Handle both object {text, sentiment} and plain string formats
                let text = q.get("text").or(q.get("content")).or(q.get("quote"))
                    .and_then(|v| v.as_str())
                    .or_else(|| q.as_str())
                    .unwrap_or("");
                let sentiment = q.get("sentiment").or(q.get("tone")).or(q.get("stance"))
                    .and_then(|v| v.as_str()).unwrap_or("");
                if !text.is_empty() {
                    let sent_str = if sentiment.is_empty() {
                        String::new()
                    } else {
                        format!("  *— {}*", sentiment)
                    };
                    md.push_str(&format!("> {}{}\n\n", text, sent_str));
                }
            }
        }
    }

    let ai_summary = if md.is_empty() {
        "分析结果解析失败".to_string()
    } else {
        md
    };

    set_progress("保存分析结果", 3, 3, false, None);

    if let Err(e) = crate::db::save_monitor_snapshot(
        &conn,
        &topic_id,
        &today,
        post_count,
        reply_count,
        &sentiment_dist,
        &top_keywords,
        &ai_summary,
        &cleaned,  // store raw parsed JSON for frontend charts
    ) {
        set_progress("error", 0, 0, true, Some(format!("保存快照失败: {}", e)));
        return;
    }

    set_progress("分析完成", 3, 3, true, None);
}

fn clean_json_response(raw: &str) -> String {
    let s = raw.trim();
    if s.starts_with("```json") {
        let inner = &s[7..];
        if let Some(end) = inner.rfind("```") {
            return inner[..end].trim().to_string();
        }
    }
    if s.starts_with("```") {
        let inner = &s[3..];
        if let Some(end) = inner.rfind("```") {
            return inner[..end].trim().to_string();
        }
    }
    s.to_string()
}

/// Normalize percentage: if value is 0-1 (fraction), multiply by 100. Returns just the number.
fn normalize_pct(v: &serde_json::Value) -> String {
    match v.as_f64() {
        Some(f) if f > 1.0 => format!("{:.0}", f),
        Some(f) => format!("{:.0}", f * 100.0),
        None => match v.as_str() {
            Some(s) => s.trim_matches('"').trim_end_matches('%').to_string(),
            None => v.to_string().trim_matches('"').to_string(),
        },
    }
}

/// Find a "positive" value from a JSON object by matching key name patterns
fn find_positive_pct(obj: &serde_json::Map<String, serde_json::Value>) -> Option<&serde_json::Value> {
    for (k, v) in obj {
        let kl = k.to_lowercase();
        if (kl.contains("positive") || kl.contains("good") || kl.contains("pos") || kl == "like")
            && (kl.contains("pct") || kl.contains("percent") || kl.contains("ratio") || kl.contains("review"))
        {
            return Some(v);
        }
    }
    None
}

/// Find a "negative" value from a JSON object by matching key name patterns
fn find_negative_pct(obj: &serde_json::Map<String, serde_json::Value>) -> Option<&serde_json::Value> {
    for (k, v) in obj {
        let kl = k.to_lowercase();
        if (kl.contains("negative") || kl.contains("bad") || kl.contains("neg"))
            && (kl.contains("pct") || kl.contains("percent") || kl.contains("ratio") || kl.contains("review"))
        {
            return Some(v);
        }
    }
    None
}

/// Parse ISO time from post detail JSON: "2025-06-02T20:30:00.000+08:00" → Unix timestamp
fn parse_iso_time(s: &str) -> Option<i64> {
    // "2025-06-02T20:30:00.000+08:00"
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp());
    }
    // "2025-06-02 20:30:00"
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(dt.and_utc().timestamp());
    }
    // "2025-06-02"
    if s.len() >= 10 {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(&s[..10], "%Y-%m-%d") {
            return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp());
        }
    }
    None
}

fn truncate_str(s: &str, max_len: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_len {
        s.to_string()
    } else {
        chars[..max_len.saturating_sub(2)].iter().collect::<String>() + ".."
    }
}
