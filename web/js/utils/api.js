/**
 * API utility functions — all fetch() calls.
 */

function qs(params) {
  return Object.entries(params)
    .filter(([_, v]) => v !== undefined && v !== null)
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`)
    .join('&');
}

export async function fetchUser(euid) {
  const res = await fetch(`/api/user?euid=${encodeURIComponent(euid)}`);
  if (!res.ok) throw new Error('获取用户信息失败');
  return res.json();
}

export async function fetchStats(euid) {
  const res = await fetch(`/api/stats?euid=${encodeURIComponent(euid)}`);
  if (!res.ok) throw new Error('获取统计数据失败');
  return res.json();
}

export async function fetchWordCloud(euid) {
  const res = await fetch(`/api/analyze/wordcloud?euid=${encodeURIComponent(euid)}`);
  if (res.ok) return res.json();
  return [];
}

export async function fetchDetailedAnalysis(euid) {
  const res = await fetch(`/api/analyze/detailed?euid=${encodeURIComponent(euid)}`);
  if (res.ok) return res.json();
  return null;
}

export async function fetchAiResult(euid) {
  const res = await fetch(`/api/analyze/ai?euid=${encodeURIComponent(euid)}`);
  if (res.ok) return res.json();
  return null;
}

export async function fetchAiPostResult(euid) {
  const res = await fetch(`/api/posts/analyze/ai?euid=${encodeURIComponent(euid)}`);
  if (res.ok) return res.json();
  return null;
}

export async function fetchSimilarity(euid, threshold) {
  const res = await fetch(`/api/analyze/similarity?euid=${encodeURIComponent(euid)}&threshold=${threshold}`);
  if (res.ok) return res.json();
  return null;
}

export async function startSimilarityAnalysis(euid, threshold) {
  const res = await fetch(`/api/analyze/similarity/start?euid=${encodeURIComponent(euid)}&threshold=${threshold}`, { method: 'POST' });
  return res.json();
}

export async function fetchAnalysisProgress(euid, threshold) {
  const res = await fetch(`/api/analyze/progress?euid=${encodeURIComponent(euid)}&threshold=${threshold}`);
  return res.json();
}

export async function startAiAnalysis(euid, apiKeyParams, force = false) {
  const res = await fetch(
    `/api/analyze/ai/start?euid=${encodeURIComponent(euid)}${apiKeyParams}${force ? '&force=true' : ''}`,
    { method: 'POST' }
  );
  return res.json();
}

export async function fetchAiProgress(euid) {
  const res = await fetch(`/api/analyze/ai-progress?euid=${encodeURIComponent(euid)}`);
  return res.json();
}

export async function startAiPostAnalysis(euid, apiKeyParams, force = false) {
  const res = await fetch(
    `/api/posts/analyze/ai/start?euid=${encodeURIComponent(euid)}${apiKeyParams}${force ? '&force=true' : ''}`,
    { method: 'POST' }
  );
  return res.json();
}

export async function fetchAiPostProgress(euid) {
  const res = await fetch(`/api/posts/analyze/ai-progress?euid=${encodeURIComponent(euid)}`);
  return res.json();
}

export async function startFetchReplies(euid, maxPages, pageSize, cookieParams, incremental = true) {
  const res = await fetch(`/api/replies/fetch?euid=${encodeURIComponent(euid)}&max_pages=${maxPages}&page_size=${pageSize}&incremental=${incremental}${cookieParams}`, { method: 'POST' });
  return res.json();
}

export async function startFetchPosts(euid, maxPages, cookieParams, incremental = true) {
  const res = await fetch(`/api/posts/fetch?euid=${encodeURIComponent(euid)}&max_pages=${maxPages}&incremental=${incremental}${cookieParams}`, { method: 'POST' });
  return res.json();
}

export async function fetchRepliesProgress(euid) {
  const res = await fetch(`/api/replies/fetch-progress?euid=${encodeURIComponent(euid)}`);
  return res.json();
}

export async function fetchPostsProgress(euid) {
  const res = await fetch(`/api/posts/fetch-progress?euid=${encodeURIComponent(euid)}`);
  return res.json();
}

export async function fetchEuidsList() {
  const res = await fetch('/api/euids');
  if (res.ok) return res.json();
  return [];
}

export async function fetchPosts(euid, limit = 200, offset = 0) {
  const res = await fetch(`/api/posts?euid=${encodeURIComponent(euid)}&limit=${limit}&offset=${offset}`);
  if (!res.ok) throw new Error('获取发帖数据失败');
  return res.json();
}

export async function fetchConfigStatus() {
  const res = await fetch('/api/config/status');
  if (res.ok) return res.json();
  return null;
}

export async function saveConfig(payload) {
  const res = await fetch('/api/config/save', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  });
  const data = await res.json();
  if (!res.ok || data.error) throw new Error(data.error || '保存失败');
  return data;
}
