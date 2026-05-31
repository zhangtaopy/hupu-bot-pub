use crate::db;
use crate::deepseek::{
    AgentTrace, ToolCallTrace, ChatMessage, TokenUsage, UserOverview,
    build_qa_tools, call_llm_with_tools, QA_TOOL_SYSTEM_PROMPT,
};
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
    activity_period: Option<String>,
}

struct ToolExecResult {
    content: String,
    summary: String,
    reply_count: usize,
    post_count: usize,
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
        "tool_calls": trace.tool_calls,
        "summary_html": trace.format_md(),
    })).unwrap_or_else(|e| {
        eprintln!("[qa] sse_round_event serialize error: {e}");
        String::new()
    })
}

fn sse_tool_call_event(round: usize, tool_name: &str, args_summary: &str, result_summary: &str) -> String {
    serde_json::to_string(&serde_json::json!({
        "type": "tool_call",
        "round": round,
        "tool_name": tool_name,
        "args_summary": args_summary,
        "result_summary": result_summary,
    })).unwrap_or_else(|e| {
        eprintln!("[qa] sse_tool_call_event serialize error: {e}");
        String::new()
    })
}

fn sse_answer_event(answer: &str, username: &str, detail: &str, usage: &TokenUsage) -> String {
    serde_json::to_string(&serde_json::json!({
        "type": "answer",
        "answer": answer,
        "username": username,
        "prompt_detail": detail,
        "prompt_tokens": usage.prompt_tokens,
        "completion_tokens": usage.completion_tokens,
        "total_tokens": usage.prompt_tokens + usage.completion_tokens,
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
    provider: &crate::deepseek::AiProvider,
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

    let (answer, traces, token_usage) = agent_loop_tools(
        db_path, http_client, provider, question, euid, &ctx, &history_ctx, Some(event_tx),
    ).await?;

    let detail = build_prompt_detail(&traces, &ctx.user_ctx_text);
    let _ = event_tx.send(sse_answer_event(&answer, &ctx.username, &detail, &token_usage)).await;

    Ok(())
}

// ── Tool-calling agent loop ──

const MAX_TOOL_TURNS: usize = 15;
const MAX_PROMPT_TOKENS: u32 = 60000;
const MAX_TOOL_RESULT_CHARS: usize = 4000;

async fn agent_loop_tools(
    db_path: &std::path::Path,
    http_client: &reqwest::Client,
    provider: &crate::deepseek::AiProvider,
    question: &str,
    euid: &str,
    ctx: &UserContext,
    history_ctx: &str,
    event_tx: Option<&tokio::sync::mpsc::Sender<String>>,
) -> anyhow::Result<(String, Vec<AgentTrace>, TokenUsage)> {
    let tools = build_qa_tools();
    let conn = db::open_db(db_path)?;

    let user_prompt = format!(
        "{}\n\n以下是你要分析的用户的基本信息（注意：这是分析对象，不是提问者）：\n\n{}\n\n---\n\n提问者的问题是：{}",
        history_ctx, ctx.user_ctx_text, question
    );

    let mut messages: Vec<ChatMessage> = vec![
        ChatMessage::System { content: QA_TOOL_SYSTEM_PROMPT.to_string() },
        ChatMessage::User { content: user_prompt },
    ];

    let mut traces: Vec<AgentTrace> = Vec::new();
    let mut total_usage = TokenUsage::default();
    let mut round = 0;
    let mut prev_prompt_tokens: u32 = 0;

    loop {
        round += 1;
        if round > MAX_TOOL_TURNS {
            let trace = AgentTrace {
                round,
                action: "达到最大轮数，生成最终回答".into(),
                reasoning: String::new(),
                keywords: vec![],
                search_tables: vec![],
                reply_count: 0,
                post_count: 0,
                tool_calls: None,
            };
            emit(event_tx, &sse_round_event(&trace)).await;
            traces.push(trace);
            break;
        }

        let response = call_llm_with_tools(http_client, provider, &messages, Some(tools.as_slice())).await?;

        let round_usage = &response.token_usage;
        let incr_prompt = if prev_prompt_tokens > 0 {
            round_usage.prompt_tokens.saturating_sub(prev_prompt_tokens)
        } else {
            round_usage.prompt_tokens
        };
        total_usage.prompt_tokens += incr_prompt;
        total_usage.completion_tokens += round_usage.completion_tokens;
        prev_prompt_tokens = round_usage.prompt_tokens;

        if round_usage.prompt_tokens > MAX_PROMPT_TOKENS {
            let trace = AgentTrace {
                round,
                action: "Token预算超限，生成最终回答".into(),
                reasoning: String::new(),
                keywords: vec![],
                search_tables: vec![],
                reply_count: 0,
                post_count: 0,
                tool_calls: None,
            };
            emit(event_tx, &sse_round_event(&trace)).await;
            traces.push(trace);
            break;
        }

        if response.tool_calls.is_empty() {
            let answer = response.content.unwrap_or_default();
            let trace = AgentTrace {
                round,
                action: "给出最终回答".into(),
                reasoning: String::new(),
                keywords: vec![],
                search_tables: vec![],
                reply_count: 0,
                post_count: 0,
                tool_calls: None,
            };
            emit(event_tx, &sse_round_event(&trace)).await;
            traces.push(trace);
            total_usage.total_tokens = total_usage.prompt_tokens + total_usage.completion_tokens;
            return Ok((answer, traces, total_usage));
        }

        let mut tool_call_traces: Vec<ToolCallTrace> = Vec::new();
        let mut total_reply_count = 0usize;
        let mut total_post_count = 0usize;

        messages.push(ChatMessage::AssistantWithToolCalls {
            content: response.content.clone(),
            tool_calls: response.tool_calls.clone(),
            reasoning_content: response.reasoning_content.clone(),
        });

        for tc in &response.tool_calls {
            let result = execute_tool(&conn, euid, ctx, &tc.function.name, &tc.function.arguments);
            total_reply_count += result.reply_count;
            total_post_count += result.post_count;

            let args_summary = truncate_args_summary(&tc.function.arguments);
            let result_summary = truncate_str(&result.summary, 100);

            tool_call_traces.push(ToolCallTrace {
                tool_name: tc.function.name.clone(),
                args_summary: args_summary.clone(),
                result_summary: result_summary.clone(),
            });

            emit(event_tx, &sse_tool_call_event(round, &tc.function.name, &args_summary, &result_summary)).await;

            let result_content = if result.content.chars().count() > MAX_TOOL_RESULT_CHARS {
                let truncated: String = result.content.chars().take(MAX_TOOL_RESULT_CHARS).collect();
                format!("{}...(结果过长，已截断)", truncated)
            } else {
                result.content
            };

            messages.push(ChatMessage::ToolResult {
                tool_call_id: tc.id.clone(),
                content: result_content,
            });
        }

        prune_messages(&mut messages, 3);

        let trace = AgentTrace {
            round,
            action: format!("调用{}个工具", tool_call_traces.len()),
            reasoning: String::new(),
            keywords: vec![],
            search_tables: vec![],
            reply_count: total_reply_count,
            post_count: total_post_count,
            tool_calls: Some(tool_call_traces),
        };
        emit(event_tx, &sse_round_event(&trace)).await;
        traces.push(trace);
    }

    let overview = build_overview(
        &conn, euid, ctx.reply_count, ctx.post_count,
        ctx.topic_vec.clone(), ctx.ai_reply_summary.clone(),
        ctx.ai_post_summary.clone(), ctx.ai_reply_personal_info.clone(),
    )?;
    let (answer, usage) = crate::deepseek::generate_answer(
        http_client, provider, question, &ctx.username, &overview,
        &[], &[], history_ctx,
    ).await?;
    total_usage.prompt_tokens += usage.prompt_tokens;
    total_usage.completion_tokens += usage.completion_tokens;
    total_usage.total_tokens = total_usage.prompt_tokens + total_usage.completion_tokens;

    Ok((answer, traces, total_usage))
}

/// Keep system + user messages and only the last `max_rounds` rounds of tool-calling history.
fn prune_messages(messages: &mut Vec<ChatMessage>, max_rounds: usize) {
    let keep_min = 2;
    if messages.len() <= keep_min + 1 {
        return;
    }
    let mut round_count = 0;
    let mut cut = messages.len();
    for i in (keep_min..messages.len()).rev() {
        if matches!(messages[i], ChatMessage::AssistantWithToolCalls { .. }) {
            round_count += 1;
            if round_count > max_rounds {
                cut = i;
                break;
            }
        }
    }
    if cut < messages.len() {
        let mut kept: Vec<ChatMessage> = messages[..keep_min].to_vec();
        kept.extend_from_slice(&messages[cut..]);
        *messages = kept;
    }
}

fn truncate_args_summary(args_json: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(args_json) else {
        return truncate_str(args_json, 80);
    };
    let mut parts = Vec::new();
    if let Some(kw) = v.get("keywords").and_then(|v| v.as_array()) {
        let s: Vec<String> = kw.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        if !s.is_empty() { parts.push(s.join(",")); }
    }
    if let Some(s) = v.get("start_date").and_then(|v| v.as_str()) {
        parts.push(format!("from {}", s));
    }
    if let Some(e) = v.get("end_date").and_then(|v| v.as_str()) {
        parts.push(format!("to {}", e));
    }
    if let Some(t) = v.get("table").and_then(|v| v.as_str()) {
        parts.push(t.to_string());
    }
    if let Some(s) = v.get("sort_by").and_then(|v| v.as_str()) {
        parts.push(format!("sort:{}", s));
    }
    if let Some(n) = v.get("limit").and_then(|v| v.as_u64()) {
        parts.push(format!("top{}", n));
    }
    if parts.is_empty() {
        truncate_str(args_json, 80)
    } else {
        parts.join(" ")
    }
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}...", truncated)
    }
}

fn execute_tool(
    conn: &rusqlite::Connection,
    euid: &str,
    ctx: &UserContext,
    tool_name: &str,
    arguments_json: &str,
) -> ToolExecResult {
    let args: serde_json::Value = serde_json::from_str(arguments_json)
        .unwrap_or(serde_json::Value::Null);

    match tool_name {
        "search_by_keywords" => execute_search_by_keywords(conn, euid, &args),
        "search_by_time_range" => execute_search_by_time_range(conn, euid, &args),
        "get_topic_stats" => execute_get_topic_stats(conn, euid, ctx),
        "get_hot_content" => execute_get_hot_content(conn, euid, &args),
        "get_user_stats" => execute_get_user_stats(conn, euid, ctx),
        "get_ai_profile" => execute_get_ai_profile(conn, euid, ctx),
        _ => ToolExecResult {
            content: "未知工具".to_string(),
            summary: "未知工具".to_string(),
            reply_count: 0,
            post_count: 0,
        },
    }
}

fn execute_search_by_keywords(
    conn: &rusqlite::Connection,
    euid: &str,
    args: &serde_json::Value,
) -> ToolExecResult {
    let keywords: Vec<String> = args.get("keywords")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    if keywords.is_empty() {
        return ToolExecResult {
            content: "请提供搜索关键词".to_string(),
            summary: "未提供关键词".to_string(),
            reply_count: 0,
            post_count: 0,
        };
    }

    let tables: Vec<String> = args.get("tables")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let topic_filter: Vec<String> = args.get("topic_filter")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let sort_by = args.get("sort_by").and_then(|v| v.as_str()).unwrap_or("relevance");
    let max_results = args.get("max_results").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

    let search_replies = tables.is_empty() || tables.iter().any(|t| t == "replies");
    let search_posts = tables.is_empty() || tables.iter().any(|t| t == "posts");

    let mut reply_count = 0;
    let mut post_count = 0;
    let mut content_parts = Vec::new();

    if search_replies {
        match db::search_replies(conn, euid, &keywords, &topic_filter, sort_by, max_results) {
            Ok(replies) => {
                reply_count = replies.len();
                content_parts.push(crate::deepseek::format_search_results_summary(&replies, &[]));
            }
            Err(e) => content_parts.push(format!("搜索回帖失败: {}", e)),
        }
    }

    if search_posts {
        match db::search_posts(conn, euid, &keywords, &topic_filter, sort_by, max_results) {
            Ok(posts) => {
                post_count = posts.len();
                content_parts.push(crate::deepseek::format_search_results_summary(&[], &posts));
            }
            Err(e) => content_parts.push(format!("搜索发帖失败: {}", e)),
        }
    }

    let content = content_parts.join("\n\n");
    let summary = format!("关键词搜索: {} (回帖:{}, 发帖:{})", keywords.join(","), reply_count, post_count);

    ToolExecResult { content, summary, reply_count, post_count }
}

fn execute_search_by_time_range(
    conn: &rusqlite::Connection,
    euid: &str,
    args: &serde_json::Value,
) -> ToolExecResult {
    let start_date = args.get("start_date").and_then(|v| v.as_str()).unwrap_or("2020-01");
    let end_date = args.get("end_date").and_then(|v| v.as_str()).unwrap_or("2030-12");
    let tables: Vec<String> = args.get("tables")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let max_results = args.get("max_results").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

    let search_replies = tables.is_empty() || tables.iter().any(|t| t == "replies");
    let search_posts = tables.is_empty() || tables.iter().any(|t| t == "posts");

    let mut reply_count = 0;
    let mut post_count = 0;
    let mut content_parts = Vec::new();

    if search_replies {
        match db::search_replies_by_time(conn, euid, start_date, end_date, max_results) {
            Ok(replies) => {
                reply_count = replies.len();
                content_parts.push(crate::deepseek::format_search_results_summary(&replies, &[]));
            }
            Err(e) => content_parts.push(format!("按时间搜索回帖失败: {}", e)),
        }
    }

    if search_posts {
        match db::search_posts_by_time(conn, euid, start_date, end_date, max_results) {
            Ok(posts) => {
                post_count = posts.len();
                content_parts.push(crate::deepseek::format_search_results_summary(&[], &posts));
            }
            Err(e) => content_parts.push(format!("按时间搜索发帖失败: {}", e)),
        }
    }

    let content = content_parts.join("\n\n");
    let summary = format!("时间范围搜索: {}~{} (回帖:{}, 发帖:{})", start_date, end_date, reply_count, post_count);

    ToolExecResult { content, summary, reply_count, post_count }
}

fn execute_get_topic_stats(
    conn: &rusqlite::Connection,
    euid: &str,
    _ctx: &UserContext,
) -> ToolExecResult {
    let reply_dist = db::query_topic_distribution(conn, euid).unwrap_or_default();
    let post_dist = db::query_post_topic_distribution(conn, euid).unwrap_or_default();

    let mut content = String::new();
    content.push_str("=== 回帖板块分布 ===\n");
    let mut reply_topics: Vec<(String, usize)> = reply_dist.into_iter().collect();
    reply_topics.sort_by(|a, b| b.1.cmp(&a.1));
    for (i, (topic, count)) in reply_topics.iter().enumerate() {
        content.push_str(&format!("{}. {} - {}条\n", i + 1, topic, count));
    }

    let post_dist_len = post_dist.len();
    if !post_dist.is_empty() {
        content.push_str("\n=== 发帖板块分布 ===\n");
        let mut post_topics: Vec<(String, usize)> = post_dist.into_iter().collect();
        post_topics.sort_by(|a, b| b.1.cmp(&a.1));
        for (i, (topic, count)) in post_topics.iter().enumerate() {
            content.push_str(&format!("{}. {} - {}条\n", i + 1, topic, count));
        }
    } else {
        drop(post_dist);
    }

    let summary = format!("板块分布统计 - 回帖板块{}个, 发帖板块{}个", reply_topics.len(), post_dist_len);
    ToolExecResult { content, summary, reply_count: 0, post_count: 0 }
}

fn execute_get_hot_content(
    conn: &rusqlite::Connection,
    euid: &str,
    args: &serde_json::Value,
) -> ToolExecResult {
    let table = args.get("table").and_then(|v| v.as_str()).unwrap_or("replies");
    let sort_by = args.get("sort_by").and_then(|v| v.as_str()).unwrap_or("lights");
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

    match table {
        "posts" => {
            match db::get_hot_posts(conn, euid, sort_by, limit) {
                Ok(posts) => {
                    let post_count = posts.len();
                    let content = crate::deepseek::format_search_results_summary(&[], &posts);
                    let summary = format!("热门发帖 top{} (按{})", post_count, sort_by);
                    ToolExecResult { content, summary, reply_count: 0, post_count }
                }
                Err(e) => ToolExecResult {
                    content: format!("获取热门发帖失败: {}", e),
                    summary: "热门发帖查询失败".to_string(),
                    reply_count: 0, post_count: 0,
                },
            }
        }
        _ => {
            match db::get_hot_replies(conn, euid, sort_by, limit) {
                Ok(replies) => {
                    let reply_count = replies.len();
                    let content = crate::deepseek::format_search_results_summary(&replies, &[]);
                    let summary = format!("热门回帖 top{} (按{})", reply_count, sort_by);
                    ToolExecResult { content, summary, reply_count, post_count: 0 }
                }
                Err(e) => ToolExecResult {
                    content: format!("获取热门回帖失败: {}", e),
                    summary: "热门回帖查询失败".to_string(),
                    reply_count: 0, post_count: 0,
                },
            }
        }
    }
}

fn execute_get_user_stats(
    conn: &rusqlite::Connection,
    euid: &str,
    ctx: &UserContext,
) -> ToolExecResult {
    let time_dist: Vec<(String, usize)> = db::query_time_distribution(conn, euid)
        .unwrap_or_default()
        .into_iter()
        .collect();

    let mut content = format!(
        "=== 用户综合统计 ===\n用户名: {}\n总回帖: {}条\n总发帖: {}条\n",
        ctx.username, ctx.reply_count, ctx.post_count
    );

    if let Some(ref period) = ctx.activity_period {
        content.push_str(&format!("活跃时间范围: {}\n", period));
    }

    if !time_dist.is_empty() {
        content.push_str("\n月度回帖分布:\n");
        for (month, count) in &time_dist {
            content.push_str(&format!("  {}: {}条\n", month, count));
        }
    }

    let reply_dist = db::query_topic_distribution(conn, euid).unwrap_or_default();
    let mut reply_topics: Vec<(String, usize)> = reply_dist.into_iter().collect();
    reply_topics.sort_by(|a, b| b.1.cmp(&a.1));
    if !reply_topics.is_empty() {
        content.push_str("\n回帖板块分布:\n");
        for (topic, count) in reply_topics.iter().take(10) {
            content.push_str(&format!("  {} - {}条\n", topic, count));
        }
    }

    let summary = format!("综合统计: {}条回帖, {}条发帖", ctx.reply_count, ctx.post_count);
    ToolExecResult { content, summary, reply_count: ctx.reply_count as usize, post_count: ctx.post_count as usize }
}

fn execute_get_ai_profile(
    conn: &rusqlite::Connection,
    euid: &str,
    ctx: &UserContext,
) -> ToolExecResult {
    let mut content = String::new();

    if let Some(ref info) = ctx.ai_reply_personal_info {
        content.push_str(&format!("=== 已推断的用户信息 ===\n{}\n", info));
    } else {
        content.push_str("=== 已推断的用户信息 ===\n（无AI分析数据，请先进行AI分析）\n");
    }

    if let Some(ref summary) = ctx.ai_reply_summary {
        content.push_str(&format!("\n=== AI回帖分析评语 ===\n{}\n", summary));
    }

    if let Some(ref summary) = ctx.ai_post_summary {
        content.push_str(&format!("\n=== AI发帖分析评语 ===\n{}\n", summary));
    }

    // Also try to get full AI analysis for more detail
    if let Ok(Some(json_str)) = db::get_ai_analysis(conn, euid) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json_str) {
            if let Some(viewpoint) = val.get("viewpoint_summary") {
                content.push_str(&format!("\n=== 观点总结 ===\n{}\n", serde_json::to_string_pretty(viewpoint).unwrap_or_default()));
            }
            if let Some(behavioral) = val.get("behavioral_patterns") {
                content.push_str(&format!("\n=== 行为模式 ===\n{}\n", serde_json::to_string_pretty(behavioral).unwrap_or_default()));
            }
        }
    }

    let has_profile = ctx.ai_reply_personal_info.is_some() || ctx.ai_reply_summary.is_some();
    let summary = if has_profile { "AI画像数据已获取" } else { "无AI画像数据" };

    ToolExecResult { content, summary: summary.to_string(), reply_count: 0, post_count: 0 }
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

async fn emit(tx: Option<&tokio::sync::mpsc::Sender<String>>, data: &str) {
    if let Some(tx) = tx {
        if tx.send(data.to_string()).await.is_err() {
            eprintln!("[qa] emit error: channel closed");
        }
    }
}

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

    let activity_period = compute_activity_period(conn, euid);

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
        activity_period,
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

