use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::task::JoinSet;

use crate::deepseek::AiProvider;
use crate::server::types::{AppState, ProgressState};

/// 回帖画像分析（增量优先）：
/// - `force_full = false` 且已有缓存游标：只分析 `fetched_at > 游标` 的新回帖，
///   再与旧画像增量合并（1 次合成调用），避免全量重分析。
/// - 无游标（从未分析）或无旧画像可合并时，回退全量分析。
/// - `force_full = true`：忽略游标，全量重分析（“重新分析”按钮）。
pub async fn run_ai_analysis_background(
    state: Arc<AppState>,
    euid: String,
    user_provider: Option<AiProvider>,
    force_full: bool,
) {
    let key = format!("ai:{}", euid);

    let set_progress = |s: &AppState, phase: &str, current: usize, total: usize, done: bool, error: Option<String>| {
        if let Ok(mut p) = s.progress.lock() {
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

    set_progress(&state, "读取数据中", 0, 0, false, None);

    let conn = match crate::db::open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("数据库打开失败: {}", e)));
            return;
        }
    };

    let total = match crate::db::count_replies(&conn, Some(&euid)) {
        Ok(t) => t,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("查询失败: {}", e)));
            return;
        }
    };

    if total == 0 {
        set_progress(&state, "error", 0, 0, true, Some("该用户没有回帖数据".into()));
        return;
    }

    // 发帖上下文（全量批次与增量批次共用）
    let posts_total = crate::db::count_posts(&conn, Some(&euid)).unwrap_or(0);
    let all_posts = if posts_total > 0 {
        crate::db::query_posts(&conn, Some(&euid), posts_total as usize, 0).unwrap_or_default()
    } else {
        Vec::new()
    };
    let posts_context = crate::deepseek::format_posts_context(&all_posts);
    let posts_ctx_opt = if posts_context.is_empty() {
        None
    } else {
        Some(posts_context)
    };

    let (provider, max_concurrency) = match crate::resolver::resolve_ai_provider_with_concurrency(user_provider) {
        Ok(p) => p,
        Err(e) => { set_progress(&state, "error", 0, 0, true, Some(e.into())); return; }
    };
    let client = state.http_client.clone();

    // ── 增量分支（行级未分析标记）──
    // 已分析过的行打 ai_analyzed 标记，增量只处理 ai_analyzed = 0 的行；
    // 不受重复抓取刷新 fetched_at 的影响，也不会漏掉补抓的老内容。
    let unanalyzed = crate::db::count_unanalyzed_replies(&conn, &euid).unwrap_or(0);
    let has_cache = crate::db::get_ai_analysis(&conn, &euid)
        .map(|o| o.is_some())
        .unwrap_or(false);

    if !force_full && unanalyzed == 0 && has_cache {
        // 无新内容且已有结果 → 直接使用缓存
        set_progress(&state, "完成（数据无更新，直接使用已有分析结果）", 0, 0, true, None);
        return;
    }

    if !force_full && unanalyzed > 0 {
        match try_incremental_reply_analysis(
            &state, &state.db_path, &client, &provider, &euid, unanalyzed as usize,
            posts_ctx_opt.as_deref(), &set_progress,
        ).await {
            Ok(true) => return,
            Ok(false) => {
                // 增量条件不满足（无可合并的旧画像）→ 落到全量
            }
            Err(e) => {
                // 增量失败且无可用结果 → 回退全量重试
                eprintln!("[ai] incremental analysis failed, falling back to full: {e}");
            }
        }
    }

    // ── 全量分支（原逻辑）──
    let all_replies = match crate::db::query_replies(&conn, Some(&euid), total as usize, 0) {
        Ok(r) => r,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("读取数据失败: {}", e)));
            return;
        }
    };

    let mut sorted = all_replies.clone();
    sorted.sort_by(|a, b| a.create_time.cmp(&b.create_time));
    let chunks = crate::deepseek::chunk_replies(&sorted);
    let total_chunks = chunks.len();

    set_progress(&state, "AI分批分析中", 0, total_chunks, false, None);

    let completed = Arc::new(AtomicUsize::new(0));
    let failed_count = Arc::new(AtomicUsize::new(0));
    let provider2 = provider.clone();
    let db_path = state.db_path.clone();
    let euid_clone = euid.clone();

    let mut join_set: JoinSet<(usize, anyhow::Result<(serde_json::Value, String)>)> =
        JoinSet::new();
    let mut next_idx: usize = 0;
    let initial_batch = max_concurrency.min(total_chunks);

    while next_idx < initial_batch {
        let i = next_idx;
        let chunk = chunks[i].clone();
        let provider = provider2.clone();
        let client = client.clone();
        let db_path = db_path.clone();
        let euid_c = euid_clone.clone();
        let posts_ctx = posts_ctx_opt.clone();
        join_set.spawn(async move {
            let ctx_ref = posts_ctx.as_deref();
            let result =
                crate::deepseek::analyze_batch_with_retry(&client, &provider, &chunk, ctx_ref, 1).await;
            if let Err(ref e) = result {
                if let Ok(conn) = crate::db::open_db(&db_path) {
                    let _ = crate::db::save_batch_error(
                        &conn, &euid_c, "reply", i, &e.to_string(), Some(&e.to_string()),
                    );
                }
            }
            (i, result)
        });
        next_idx += 1;
    }

    let mut batch_results: Vec<(usize, serde_json::Value)> = Vec::new();
    while let Some(join_result) = join_set.join_next().await {
        match join_result {
            Ok((i, Ok((value, _raw)))) => batch_results.push((i, value)),
            Ok((_, Err(_))) => {
                failed_count.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                failed_count.fetch_add(1, Ordering::Relaxed);
            }
        }
        let n = completed.fetch_add(1, Ordering::Relaxed) + 1;
        let failed_n = failed_count.load(Ordering::Relaxed);
        let phase = if failed_n > 0 {
            format!("AI分批分析 {}/{} (失败{}批)", n, total_chunks, failed_n)
        } else {
            format!("AI分批分析 {}/{}", n, total_chunks)
        };
        set_progress(&state, &phase, n, total_chunks, false, None);

        if next_idx < total_chunks {
            let i = next_idx;
            let chunk = chunks[i].clone();
            let provider = provider2.clone();
            let client = client.clone();
            let db_path = db_path.clone();
            let euid_c = euid_clone.clone();
            let posts_ctx = posts_ctx_opt.clone();
            join_set.spawn(async move {
                let ctx_ref = posts_ctx.as_deref();
                let result =
                    crate::deepseek::analyze_batch_with_retry(&client, &provider, &chunk, ctx_ref, 1).await;
                if let Err(ref e) = result {
                    if let Ok(conn) = crate::db::open_db(&db_path) {
                        let _ = crate::db::save_batch_error(
                            &conn, &euid_c, "reply", i, &e.to_string(), Some(&e.to_string()),
                        );
                    }
                }
                (i, result)
            });
            next_idx += 1;
        }
    }

    batch_results.sort_by_key(|(i, _)| *i);
    let failed_count = failed_count.load(Ordering::Relaxed);
    let batch_results: Vec<serde_json::Value> =
        batch_results.into_iter().map(|(_, v)| v).collect();

    if batch_results.is_empty() {
        set_progress(&state, "error", 0, 0, true, Some("所有批次分析均失败".to_string()));
        return;
    }

    let synth_phase = if failed_count > 0 {
        format!(
            "综合生成用户画像中 ({}批成功, {}批失败)",
            batch_results.len(),
            failed_count
        )
    } else {
        format!("综合生成用户画像中 ({}批完成)", batch_results.len())
    };
    set_progress(&state, &synth_phase, total_chunks, total_chunks, false, None);
    let synthesis_result = match crate::deepseek::synthesize_results(
        &client,
        &provider2,
        &batch_results,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            set_progress(
                &state,
                "error",
                total_chunks,
                total_chunks,
                true,
                Some(format!("AI综合失败: {}", e)),
            );
            return;
        }
    };

    if let Ok(mut results) = state.ai_results.lock() {
        results.insert(key.clone(), synthesis_result.clone());
    }

    if let Ok(conn) = crate::db::open_db(&state.db_path) {
        if let Ok(result_json) = serde_json::to_string(&synthesis_result) {
            let max_fetched = crate::db::max_replies_fetched_at(&conn, &euid).unwrap_or(0);
            let _ = crate::db::save_ai_analysis(&conn, &euid, &result_json, max_fetched);
        }
        // 全量分析完成 → 全部行打已分析标记（后续增量只处理新增行）
        let _ = crate::db::mark_replies_analyzed(&conn, &euid);
    }

    set_progress(&state, "完成", total_chunks, total_chunks, true, None);
}

/// 增量回帖分析：只分析该用户 `ai_analyzed = 0`（尚未被分析）的回帖，与旧画像合并。
/// 返回 Ok(true)=增量完成并保存；Ok(false)=无未分析行或无可合并的旧画像（应走全量）；
/// Err=增量流程失败（可回退全量）。
///
/// 注意：rusqlite::Connection 非 Send，不能跨 await 持有；因此连接只在
/// 读取段与保存段内短期打开，AI 调用段不持有连接。
async fn try_incremental_reply_analysis(
    state: &Arc<AppState>,
    db_path: &std::path::Path,
    client: &reqwest::Client,
    provider: &AiProvider,
    euid: &str,
    unanalyzed_count: usize,
    posts_ctx: Option<&str>,
    set_progress: &(dyn Fn(&AppState, &str, usize, usize, bool, Option<String>) + Sync),
) -> anyhow::Result<bool> {
    if unanalyzed_count == 0 {
        return Ok(false);
    }

    // 读取段：未分析行 + 旧画像（连接在此作用域结束即释放）
    let (new_rows, old_value) = {
        let conn = crate::db::open_db(db_path)?;
        let new_replies = crate::db::query_unanalyzed_replies(&conn, euid)?;
        if new_replies.is_empty() {
            return Ok(false);
        }
        let old_json = match crate::db::get_ai_analysis(&conn, euid)? {
            Some(j) => j,
            None => return Ok(false), // 无旧画像可合并 → 全量
        };
        match serde_json::from_str::<serde_json::Value>(&old_json) {
            Ok(v) => (new_replies, v),
            Err(_) => return Ok(false), // 旧画像损坏 → 全量
        }
    };

    let mut sorted = new_rows.clone();
    sorted.sort_by(|a, b| a.create_time.cmp(&b.create_time));
    let chunks = crate::deepseek::chunk_replies(&sorted);
    let total_chunks = chunks.len();

    set_progress(state, "增量提取新回帖分析中", 0, total_chunks, false, None);

    let mut new_batch_results: Vec<serde_json::Value> = Vec::new();
    let mut failed = 0usize;
    for (i, chunk) in chunks.iter().enumerate() {
        match crate::deepseek::analyze_batch_with_retry(client, provider, chunk, posts_ctx, 1).await {
            Ok((value, _raw)) => new_batch_results.push(value),
            Err(e) => {
                failed += 1;
                if let Ok(conn) = crate::db::open_db(db_path) {
                    let _ = crate::db::save_batch_error(&conn, euid, "reply", i, &e.to_string(), Some(&e.to_string()));
                }
            }
        }
        let n = i + 1;
        let phase = if failed > 0 {
            format!("增量分析 {}/{} (失败{}块)", n, total_chunks, failed)
        } else {
            format!("增量分析 {}/{}", n, total_chunks)
        };
        set_progress(state, &phase, n, total_chunks, false, None);
    }

    if new_batch_results.is_empty() {
        anyhow::bail!("增量批次全部失败（{}块）", total_chunks);
    }

    set_progress(state, "与旧画像增量合并中", total_chunks, total_chunks, false, None);
    let synthesis_result = crate::deepseek::synthesize_incremental(
        client, provider, &old_value, &new_batch_results,
    ).await?;

    // 更新内存缓存（键在调用方已构造，这里用 euid 重建同一 key）
    let key = format!("ai:{}", euid);
    if let Ok(mut results) = state.ai_results.lock() {
        results.insert(key.clone(), synthesis_result.clone());
    }

    // 保存段：重开连接写回结果、游标快照，并标记这些行已分析
    if let Ok(conn) = crate::db::open_db(db_path) {
        if let Ok(result_json) = serde_json::to_string(&synthesis_result) {
            let max_fetched = crate::db::max_replies_fetched_at(&conn, euid).unwrap_or(0);
            let _ = crate::db::save_ai_analysis(&conn, euid, &result_json, max_fetched);
        }
        let _ = crate::db::mark_replies_analyzed(&conn, euid);
    }

    set_progress(
        state,
        &format!("完成（增量分析 {} 条新回帖）", new_rows.len()),
        total_chunks, total_chunks, true, None,
    );
    Ok(true)
}

/// 发帖画像分析（增量优先）：语义与 run_ai_analysis_background 相同。
pub async fn run_ai_post_analysis_background(
    state: Arc<AppState>,
    euid: String,
    user_provider: Option<AiProvider>,
    force_full: bool,
) {
    let key = format!("ai_post:{}", euid);

    let set_progress = |s: &AppState, phase: &str, current: usize, total: usize, done: bool, error: Option<String>| {
        if let Ok(mut p) = s.progress.lock() {
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

    set_progress(&state, "读取发帖数据中", 0, 0, false, None);

    let conn = match crate::db::open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("数据库打开失败: {}", e)));
            return;
        }
    };

    let total = match crate::db::count_posts(&conn, Some(&euid)) {
        Ok(t) => t,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("查询失败: {}", e)));
            return;
        }
    };

    if total == 0 {
        set_progress(&state, "error", 0, 0, true, Some("该用户没有发帖数据".into()));
        return;
    }

    let (provider, max_concurrency) = match crate::resolver::resolve_ai_provider_with_concurrency(user_provider) {
        Ok(p) => p,
        Err(e) => { set_progress(&state, "error", 0, 0, true, Some(e.into())); return; }
    };
    let client = state.http_client.clone();

    // ── 增量分支（行级未分析标记）──
    let unanalyzed = crate::db::count_unanalyzed_posts(&conn, &euid).unwrap_or(0);
    let has_cache = crate::db::get_ai_post_analysis(&conn, &euid)
        .map(|o| o.is_some())
        .unwrap_or(false);

    if !force_full && unanalyzed == 0 && has_cache {
        set_progress(&state, "完成（数据无更新，直接使用已有分析结果）", 0, 0, true, None);
        return;
    }

    if !force_full && unanalyzed > 0 {
        match try_incremental_post_analysis(
            &state, &state.db_path, &client, &provider, &euid, unanalyzed as usize, &set_progress,
        ).await {
            Ok(true) => return,
            Ok(false) => {
                // 增量条件不满足（无可合并的旧画像）→ 落到全量
            }
            Err(e) => {
                eprintln!("[ai] incremental post analysis failed, falling back to full: {e}");
            }
        }
    }

    // ── 全量分支（原逻辑）──
    let all_posts = match crate::db::query_posts(&conn, Some(&euid), total as usize, 0) {
        Ok(r) => r,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("读取数据失败: {}", e)));
            return;
        }
    };

    let mut sorted = all_posts.clone();
    sorted.sort_by(|a, b| a.create_time.cmp(&b.create_time));
    let chunks = crate::deepseek::chunk_posts(&sorted);
    let total_chunks = chunks.len();

    set_progress(&state, "AI分批分析帖子中", 0, total_chunks, false, None);

    let completed = Arc::new(AtomicUsize::new(0));
    let failed_count = Arc::new(AtomicUsize::new(0));
    let provider2 = provider.clone();
    let db_path = state.db_path.clone();
    let euid_clone = euid.clone();

    let mut join_set: JoinSet<(usize, anyhow::Result<(serde_json::Value, String)>)> =
        JoinSet::new();
    let mut next_idx: usize = 0;
    let initial_batch = max_concurrency.min(total_chunks);

    while next_idx < initial_batch {
        let i = next_idx;
        let chunk = chunks[i].clone();
        let provider = provider2.clone();
        let client = client.clone();
        let db_path = db_path.clone();
        let euid_c = euid_clone.clone();
        join_set.spawn(async move {
            let result =
                crate::deepseek::analyze_post_batch_with_retry(&client, &provider, &chunk, 1).await;
            if let Err(ref e) = result {
                if let Ok(conn) = crate::db::open_db(&db_path) {
                    let _ = crate::db::save_batch_error(
                        &conn, &euid_c, "post", i, &e.to_string(), Some(&e.to_string()),
                    );
                }
            }
            (i, result)
        });
        next_idx += 1;
    }

    let mut batch_results: Vec<(usize, serde_json::Value)> = Vec::new();
    while let Some(join_result) = join_set.join_next().await {
        match join_result {
            Ok((i, Ok((value, _raw)))) => batch_results.push((i, value)),
            Ok((_, Err(_))) => {
                failed_count.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                failed_count.fetch_add(1, Ordering::Relaxed);
            }
        }
        let n = completed.fetch_add(1, Ordering::Relaxed) + 1;
        let failed_n = failed_count.load(Ordering::Relaxed);
        let phase = if failed_n > 0 {
            format!("AI分批分析帖子 {}/{} (失败{}批)", n, total_chunks, failed_n)
        } else {
            format!("AI分批分析帖子 {}/{}", n, total_chunks)
        };
        set_progress(&state, &phase, n, total_chunks, false, None);

        if next_idx < total_chunks {
            let i = next_idx;
            let chunk = chunks[i].clone();
            let provider = provider2.clone();
            let client = client.clone();
            let db_path = db_path.clone();
            let euid_c = euid_clone.clone();
            join_set.spawn(async move {
                let result =
                    crate::deepseek::analyze_post_batch_with_retry(&client, &provider, &chunk, 1).await;
                if let Err(ref e) = result {
                    if let Ok(conn) = crate::db::open_db(&db_path) {
                        let _ = crate::db::save_batch_error(
                            &conn, &euid_c, "post", i, &e.to_string(), Some(&e.to_string()),
                        );
                    }
                }
                (i, result)
            });
            next_idx += 1;
        }
    }

    batch_results.sort_by_key(|(i, _)| *i);
    let failed_count = failed_count.load(Ordering::Relaxed);
    let batch_results: Vec<serde_json::Value> =
        batch_results.into_iter().map(|(_, v)| v).collect();

    if batch_results.is_empty() {
        set_progress(&state, "error", 0, 0, true, Some("所有批次分析均失败".to_string()));
        return;
    }

    let synth_phase = if failed_count > 0 {
        format!(
            "综合生成发帖分析画像中 ({}批成功, {}批失败)",
            batch_results.len(),
            failed_count
        )
    } else {
        format!("综合生成发帖分析画像中 ({}批完成)", batch_results.len())
    };
    set_progress(&state, &synth_phase, total_chunks, total_chunks, false, None);
    let synthesis_result = match crate::deepseek::synthesize_post_results(
        &client,
        &provider2,
        &batch_results,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            set_progress(
                &state,
                "error",
                total_chunks,
                total_chunks,
                true,
                Some(format!("AI综合失败: {}", e)),
            );
            return;
        }
    };

    if let Ok(mut results) = state.ai_post_results.lock() {
        results.insert(key.clone(), synthesis_result.clone());
    }

    if let Ok(conn) = crate::db::open_db(&state.db_path) {
        if let Ok(result_json) = serde_json::to_string(&synthesis_result) {
            let max_fetched = crate::db::max_posts_fetched_at(&conn, &euid).unwrap_or(0);
            let _ = crate::db::save_ai_post_analysis(&conn, &euid, &result_json, max_fetched);
        }
        // 全量分析完成 → 全部行打已分析标记（后续增量只处理新增行）
        let _ = crate::db::mark_posts_analyzed(&conn, &euid);
    }

    set_progress(&state, "完成", total_chunks, total_chunks, true, None);
}

/// 增量发帖分析：只分析该用户 `ai_analyzed = 0`（尚未被分析）的发帖，与旧画像合并。
/// 返回 Ok(true)=增量完成并保存；Ok(false)=无未分析行或无可合并的旧画像（应走全量）；
/// Err=增量流程失败（可回退全量）。
async fn try_incremental_post_analysis(
    state: &Arc<AppState>,
    db_path: &std::path::Path,
    client: &reqwest::Client,
    provider: &AiProvider,
    euid: &str,
    unanalyzed_count: usize,
    set_progress: &(dyn Fn(&AppState, &str, usize, usize, bool, Option<String>) + Sync),
) -> anyhow::Result<bool> {
    if unanalyzed_count == 0 {
        return Ok(false);
    }

    // 读取段：未分析行 + 旧画像（连接在此作用域结束即释放）
    let (new_rows, old_value) = {
        let conn = crate::db::open_db(db_path)?;
        let new_posts = crate::db::query_unanalyzed_posts(&conn, euid)?;
        if new_posts.is_empty() {
            return Ok(false);
        }
        let old_json = match crate::db::get_ai_post_analysis(&conn, euid)? {
            Some(j) => j,
            None => return Ok(false),
        };
        match serde_json::from_str::<serde_json::Value>(&old_json) {
            Ok(v) => (new_posts, v),
            Err(_) => return Ok(false),
        }
    };

    let mut sorted = new_rows.clone();
    sorted.sort_by(|a, b| a.create_time.cmp(&b.create_time));
    let chunks = crate::deepseek::chunk_posts(&sorted);
    let total_chunks = chunks.len();

    set_progress(state, "增量提取新发帖分析中", 0, total_chunks, false, None);

    let mut new_batch_results: Vec<serde_json::Value> = Vec::new();
    let mut failed = 0usize;
    for (i, chunk) in chunks.iter().enumerate() {
        match crate::deepseek::analyze_post_batch_with_retry(client, provider, chunk, 1).await {
            Ok((value, _raw)) => new_batch_results.push(value),
            Err(e) => {
                failed += 1;
                if let Ok(conn) = crate::db::open_db(db_path) {
                    let _ = crate::db::save_batch_error(&conn, euid, "post", i, &e.to_string(), Some(&e.to_string()));
                }
            }
        }
        let n = i + 1;
        let phase = if failed > 0 {
            format!("增量分析发帖 {}/{} (失败{}块)", n, total_chunks, failed)
        } else {
            format!("增量分析发帖 {}/{}", n, total_chunks)
        };
        set_progress(state, &phase, n, total_chunks, false, None);
    }

    if new_batch_results.is_empty() {
        anyhow::bail!("增量发帖批次全部失败（{}块）", total_chunks);
    }

    set_progress(state, "与旧发帖画像增量合并中", total_chunks, total_chunks, false, None);
    let synthesis_result = crate::deepseek::synthesize_post_incremental(
        client, provider, &old_value, &new_batch_results,
    ).await?;

    let key = format!("ai_post:{}", euid);
    if let Ok(mut results) = state.ai_post_results.lock() {
        results.insert(key.clone(), synthesis_result.clone());
    }

    // 保存段：重开连接写回结果、游标快照，并标记这些行已分析
    if let Ok(conn) = crate::db::open_db(db_path) {
        if let Ok(result_json) = serde_json::to_string(&synthesis_result) {
            let max_fetched = crate::db::max_posts_fetched_at(&conn, euid).unwrap_or(0);
            let _ = crate::db::save_ai_post_analysis(&conn, euid, &result_json, max_fetched);
        }
        let _ = crate::db::mark_posts_analyzed(&conn, euid);
    }

    set_progress(
        state,
        &format!("完成（增量更新 {} 条新发帖）", new_rows.len()),
        total_chunks, total_chunks, true, None,
    );
    Ok(true)
}