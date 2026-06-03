use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::task::JoinSet;

use crate::deepseek::AiProvider;
use crate::server::types::{AppState, ProgressState};

pub async fn run_ai_analysis_background(state: Arc<AppState>, euid: String, user_provider: Option<AiProvider>) {
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

    let all_replies = match crate::db::query_replies(&conn, Some(&euid), total as usize, 0) {
        Ok(r) => r,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("读取数据失败: {}", e)));
            return;
        }
    };

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

    let mut sorted = all_replies.clone();
    sorted.sort_by(|a, b| a.create_time.cmp(&b.create_time));
    let chunks = crate::deepseek::chunk_replies(&sorted);
    let total_chunks = chunks.len();

    set_progress(&state, "AI分批分析中", 0, total_chunks, false, None);

    let completed = Arc::new(AtomicUsize::new(0));
    let failed_count = Arc::new(AtomicUsize::new(0));
    let provider2 = provider.clone();
    let client = state.http_client.clone();
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
            let _ = crate::db::save_ai_analysis(&conn, &euid, &result_json);
        }
    }

    set_progress(&state, "完成", total_chunks, total_chunks, true, None);
}

pub async fn run_ai_post_analysis_background(state: Arc<AppState>, euid: String, user_provider: Option<AiProvider>) {
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

    let all_posts = match crate::db::query_posts(&conn, Some(&euid), total as usize, 0) {
        Ok(r) => r,
        Err(e) => {
            set_progress(&state, "error", 0, 0, true, Some(format!("读取数据失败: {}", e)));
            return;
        }
    };

    let (provider, max_concurrency) = match crate::resolver::resolve_ai_provider_with_concurrency(user_provider) {
        Ok(p) => p,
        Err(e) => { set_progress(&state, "error", 0, 0, true, Some(e.into())); return; }
    };

    let mut sorted = all_posts.clone();
    sorted.sort_by(|a, b| a.create_time.cmp(&b.create_time));
    let chunks = crate::deepseek::chunk_posts(&sorted);
    let total_chunks = chunks.len();

    set_progress(&state, "AI分批分析帖子中", 0, total_chunks, false, None);

    let completed = Arc::new(AtomicUsize::new(0));
    let failed_count = Arc::new(AtomicUsize::new(0));
    let provider2 = provider.clone();
    let client = state.http_client.clone();
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
            let _ = crate::db::save_ai_post_analysis(&conn, &euid, &result_json);
        }
    }

    set_progress(&state, "完成", total_chunks, total_chunks, true, None);
}
