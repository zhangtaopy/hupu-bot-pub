// ── AI 玩法：成分卡（查成分）+ 魂穿（克隆人/嘴替/对线模拟） ──
//
// 成分卡：基于用户回帖/发帖生成结构化"成分卡"，结果缓存到 ghost_profile 表。
// 魂穿：取用户最近回帖作为风格样本，AI 模仿该用户口吻生成回复/对话。

use crate::deepseek::{AiProvider, ChatMessage};

const STYLE_SAMPLE_COUNT: usize = 40;
const PROFILE_REPLY_SAMPLE_COUNT: usize = 50;
const MAX_HISTORY_ROUNDS: usize = 6;

// ── 成分卡 ──

/// 读取成分卡所需的全部输入数据（用户名、回帖样本、发帖样本、统计摘要）。
fn load_profile_inputs(
    conn: &rusqlite::Connection,
    euid: &str,
) -> anyhow::Result<(String, String, String, String)> {
    let reply_count = crate::db::count_replies(conn, Some(euid))?;
    let post_count = crate::db::count_posts(conn, Some(euid))?;
    if reply_count == 0 && post_count == 0 {
        anyhow::bail!("该用户没有回帖和发帖数据，请先获取数据");
    }

    let username = crate::db::get_username(conn, euid)?
        .unwrap_or_else(|| "未知用户".to_string());

    // 最近回帖样本（按时间倒序）
    let recent_replies = crate::db::query_replies(conn, Some(euid), PROFILE_REPLY_SAMPLE_COUNT, 0)?;
    let style_samples = crate::deepseek::build_style_samples(&recent_replies, STYLE_SAMPLE_COUNT);

    // 发帖样本
    let posts = crate::db::query_posts(conn, Some(euid), 20, 0).unwrap_or_default();
    let posts_context = crate::deepseek::format_posts_context(&posts);

    // 统计摘要
    let topic_dist = crate::db::query_topic_distribution(conn, euid).unwrap_or_default();
    let mut topic_vec: Vec<(String, usize)> = topic_dist.into_iter().collect();
    topic_vec.sort_by(|a, b| b.1.cmp(&a.1));
    topic_vec.truncate(5);
    let topics_str = if topic_vec.is_empty() {
        "无".to_string()
    } else {
        topic_vec.iter()
            .map(|(t, c)| format!("{}×{}", t, c))
            .collect::<Vec<_>>()
            .join("，")
    };

    let stats_summary = format!(
        "用户名: {}\n总回帖: {}条\n总发帖: {}条\n板块分布: {}",
        username, reply_count, post_count, topics_str
    );

    Ok((username, style_samples, posts_context, stats_summary))
}

/// 生成用户的成分卡；`force_refresh = true` 时跳过缓存重新生成。
/// 返回 `(卡片JSON, 是否命中缓存)`。
pub async fn build_profile_card(
    db_path: &std::path::Path,
    http_client: &reqwest::Client,
    provider: &AiProvider,
    euid: &str,
    force_refresh: bool,
) -> anyhow::Result<(serde_json::Value, bool)> {
    let conn = crate::db::open_db(db_path)?;

    // 缓存命中直接返回（强制刷新时跳过）
    if !force_refresh {
        if let Ok(Some(cached)) = crate::db::get_ghost_profile(&conn, euid) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&cached) {
                return Ok((v, true));
            }
        }
    }

    let (_username, style_samples, posts_context, stats_summary) = load_profile_inputs(&conn, euid)?;

    let (value, _raw) = crate::deepseek::generate_profile_card(
        http_client, provider, &style_samples, &posts_context, &stats_summary,
    ).await?;

    // 写入缓存（失败不影响返回）
    if let Ok(json_str) = serde_json::to_string(&value) {
        if let Err(e) = crate::db::save_ghost_profile(&conn, euid, &json_str) {
            eprintln!("[ghost] save profile failed: {e}");
        }
    }

    Ok((value, false))
}

/// 流式生成成分卡：通过 `event_tx` 发送 NDJSON 事件。
/// 事件：`{type:"stage", stage:"..."}` 阶段进度；`{type:"done", cached, result}` 完成；
/// `{type:"error", error}` 失败。缓存命中时立即发 done。
pub async fn stream_profile_card(
    db_path: &std::path::Path,
    http_client: &reqwest::Client,
    provider: &AiProvider,
    euid: &str,
    force_refresh: bool,
    event_tx: &tokio::sync::mpsc::Sender<String>,
) -> anyhow::Result<()> {
    let send = |v: serde_json::Value| {
        let tx = event_tx.clone();
        async move {
            let _ = tx.send(serde_json::to_string(&v).unwrap_or_default()).await;
        }
    };

    let conn = crate::db::open_db(db_path)?;

    // 缓存命中直接返回
    if !force_refresh {
        if let Ok(Some(cached)) = crate::db::get_ghost_profile(&conn, euid) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&cached) {
                send(serde_json::json!({ "type": "done", "cached": true, "result": v })).await;
                return Ok(());
            }
        }
    }

    // 阶段 1：读取数据
    send(serde_json::json!({ "type": "stage", "stage": "读取用户数据" })).await;
    if event_tx.is_closed() {
        return Ok(());
    }
    let (_username, style_samples, posts_context, stats_summary) = load_profile_inputs(&conn, euid)?;

    // 阶段 2：AI 分析
    send(serde_json::json!({ "type": "stage", "stage": "AI 分析中" })).await;
    if event_tx.is_closed() {
        return Ok(());
    }
    let (value, _raw) = crate::deepseek::generate_profile_card(
        http_client, provider, &style_samples, &posts_context, &stats_summary,
    ).await?;

    // 写入缓存（失败不影响返回）
    if let Ok(json_str) = serde_json::to_string(&value) {
        if let Err(e) = crate::db::save_ghost_profile(&conn, euid, &json_str) {
            eprintln!("[ghost] save profile failed: {e}");
        }
    }

    send(serde_json::json!({ "type": "done", "cached": false, "result": value })).await;
    Ok(())
}

// ── 魂穿 ──

/// CLI 文本输出：把成分卡 JSON 打印为可读文本。
pub fn print_profile_card(card: &serde_json::Value) {
    let s = |k: &str| card.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let arr = |k: &str| -> Vec<String> {
        card.get(k).and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default()
    };

    println!("{}", "=".repeat(48));
    println!("  虎扑成分卡 — {}", s("username"));
    println!("{}", "=".repeat(48));
    println!("主混迹板块 : {}", s("main_forum"));
    println!("粉籍判断   : {}", s("fan_identity"));
    println!("抽象指数   : {}", s("abstract_index"));
    println!("水贴指数   : {}", s("water_index"));
    println!("点亮质量   : {}", s("light_quality"));
    let phrases = arr("catchphrases");
    if !phrases.is_empty() {
        println!("口头禅     : {}", phrases.join(" / "));
    }
    let quotes = arr("classic_quotes");
    if !quotes.is_empty() {
        println!("经典语录   :");
        for q in &quotes {
            println!("  · {}", q);
        }
    }
    let black = arr("black_history");
    if !black.is_empty() {
        println!("历史黑点   :");
        for b in &black {
            println!("  · {}", b);
        }
    }
    let specs = arr("specialties");
    if !specs.is_empty() {
        println!("核心特长   : {}", specs.join(" / "));
    }
    let summary = s("summary");
    if !summary.is_empty() {
        println!("总结       : {}", summary);
    }
    println!("{}", "=".repeat(48));
}

/// 流式魂穿：通过 `event_tx` 发送 NDJSON 事件（start / answer / error）。
#[allow(clippy::too_many_arguments)]
pub async fn run_ghost_chat_streaming(
    db_path: &std::path::Path,
    http_client: &reqwest::Client,
    provider: &AiProvider,
    euid: &str,
    mode: &str,
    content: &str,
    history: &[crate::server::types::HistoryEntry],
    event_tx: &tokio::sync::mpsc::Sender<String>,
) -> anyhow::Result<()> {
    let conn = crate::db::open_db(db_path)?;

    let reply_count = crate::db::count_replies(&conn, Some(euid))?;
    if reply_count == 0 {
        anyhow::bail!("该用户没有回帖数据，无法学习其风格，请先获取数据");
    }

    let username = crate::db::get_username(&conn, euid)?
        .unwrap_or_else(|| "未知用户".to_string());

    let recent_replies = crate::db::query_replies(&conn, Some(euid), STYLE_SAMPLE_COUNT, 0)?;
    let style_samples = crate::deepseek::build_style_samples(&recent_replies, STYLE_SAMPLE_COUNT);

    let system_prompt = crate::deepseek::build_ghost_system_prompt(&username, &style_samples, mode);

    // 组装消息：system + 最近几轮完整历史（user/assistant 交替）+ 当前输入
    let mut messages: Vec<ChatMessage> = vec![ChatMessage::System { content: system_prompt }];
    let history_len = history.len();
    let start = history_len.saturating_sub(MAX_HISTORY_ROUNDS);
    for h in &history[start..] {
        // 跳过未完成的轮次，保持 user/assistant 严格交替（部分 provider 会拒绝不配对的消息）
        if h.question.trim().is_empty() || h.answer.trim().is_empty() {
            continue;
        }
        messages.push(ChatMessage::User { content: h.question.clone() });
        messages.push(ChatMessage::Assistant { content: h.answer.clone() });
    }
    messages.push(ChatMessage::User { content: content.to_string() });

    // 发送开始事件（带用户名，前端展示"你正在扮演谁"）
    let _ = event_tx.send(serde_json::to_string(&serde_json::json!({
        "type": "start",
        "username": username,
    })).unwrap_or_default()).await;

    // 检测客户端断开
    if event_tx.is_closed() {
        return Ok(());
    }

    let response = crate::deepseek::call_llm_with_tools(http_client, provider, &messages, None).await?;
    let answer = response.content.unwrap_or_default();
    let usage = &response.token_usage;

    let _ = event_tx.send(serde_json::to_string(&serde_json::json!({
        "type": "answer",
        "answer": answer,
        "prompt_tokens": usage.prompt_tokens,
        "completion_tokens": usage.completion_tokens,
        "total_tokens": usage.prompt_tokens + usage.completion_tokens,
    })).unwrap_or_default()).await;

    Ok(())
}
