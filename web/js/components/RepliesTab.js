/**
 * Replies Tab — user replies analysis, similarity analysis, AI analysis.
 * Core analysis logic.
 */
import * as api from '../utils/api.js';

export function setupRepliesTab(store, charts, storeObj) {
  // Poll similarity analysis progress
  function pollProgress(key) {
    if (!key) return;
    api.fetchAnalysisProgress(store.euid.value, store.threshold.value).then(p => {
      if (p.phase && p.phase !== 'idle') store.progressPhase.value = p.phase;
      if (p.total > 0) {
        store.progressCurrent.value = p.current;
        store.progressTotal.value = p.total;
      }
      if (p.done) {
        if (p.error) {
          store.error.value = p.error;
          store.similarityLoading.value = false;
          store.progressPhase.value = '';
        } else {
          store.progressPhase.value = '加载相似度结果中';
          fetchSimilarityResults();
        }
        return;
      }
      setTimeout(() => pollProgress(key), 1000);
    }).catch(() => {
      setTimeout(() => pollProgress(key), 2000);
    });
  }

  async function fetchSimilarityResults() {
    try {
      const [analyzeRes, statsRes] = await Promise.all([
        api.fetchSimilarity(store.euid.value, store.threshold.value),
        api.fetchStats(store.euid.value),
      ]);
      if (analyzeRes && analyzeRes.status === 'done') {
        store.groups.value = analyzeRes.groups;
        store.hasSimilarityResults.value = true;
      }
      if (statsRes) store.stats.value = statsRes;
      store.groups.value.slice(0, 3).forEach(g => { store.expandedGroups[g.group_id] = true; });
    } catch (e) {
      store.error.value = e.message || '获取相似度结果失败';
    } finally {
      store.similarityLoading.value = false;
      store.progressPhase.value = '';
    }
  }

  function clearTimers() {
    if (store.timers.pollTimer) clearTimeout(store.timers.pollTimer);
    if (store.timers.aiPollTimer) clearTimeout(store.timers.aiPollTimer);
  }

  // Fetch all basic results (user, stats, wordcloud, detailed, ai)
  async function fetchBasicResults() {
    try {
      const [userData, statsData, wcData, detailedData, aiData] = await Promise.all([
        api.fetchUser(store.euid.value),
        api.fetchStats(store.euid.value),
        api.fetchWordCloud(store.euid.value),
        api.fetchDetailedAnalysis(store.euid.value),
        api.fetchAiResult(store.euid.value),
      ]);
      store.userInfo.value = userData;
      store.stats.value = statsData;
      store.displayedEuid.value = store.euid.value;
      store.hasSimilarityResults.value = statsData.similarity_available;

      if (statsData.similarity_available) {
        const simData = await api.fetchSimilarity(store.euid.value, store.threshold.value);
        if (simData && simData.status === 'done') {
          store.groups.value = simData.groups;
          store.groups.value.slice(0, 3).forEach(g => { store.expandedGroups[g.group_id] = true; });
        }
      }

      if (wcData) store.wordCloudWords.value = wcData;
      if (detailedData) store.detailedAnalysis.value = detailedData;
      if (aiData && aiData.status === 'done' && aiData.result) {
        store.aiResult.value = aiData.result;
      }

      await Vue.nextTick();
      charts.renderCharts();
      charts.renderWordCloud();
      charts.renderDetailedCharts();
    } catch (e) {
      store.error.value = e.message || '获取结果失败';
    } finally {
      store.loading.value = false;
    }
  }

  async function analyze() {
    if (!store.euid.value.trim()) return;
    clearTimers();
    store.loading.value = true;
    store.error.value = '';
    store.stats.value = null;
    store.groups.value = [];
    store.wordCloudWords.value = [];
    store.detailedAnalysis.value = null;
    store.aiResult.value = null;
    store.hasSimilarityResults.value = false;

    try {
      await fetchBasicResults();
    } catch (e) {
      store.error.value = e.message || '分析失败，请检查 euid 或数据库';
      store.loading.value = false;
    }
  }

  async function runSimilarity() {
    clearTimers();
    store.similarityLoading.value = true;
    store.error.value = '';
    store.progressPhase.value = '准备中';
    store.progressCurrent.value = 0;
    store.progressTotal.value = 0;

    try {
      const startData = await api.startSimilarityAnalysis(store.euid.value, store.threshold.value);
      if (startData.status === 'error') {
        store.error.value = startData.error || '相似度分析启动失败';
        store.similarityLoading.value = false;
        store.progressPhase.value = '';
        return;
      }
      pollProgress(startData.key);
    } catch (e) {
      store.error.value = e.message || '启动相似度分析失败';
      store.similarityLoading.value = false;
      store.progressPhase.value = '';
    }
  }

  // ── AI Analysis ──
  function aiPollProgress(key) {
    if (!key) return;
    api.fetchAiProgress(store.euid.value).then(p => {
      if (p.phase && p.phase !== 'idle') store.aiProgressPhase.value = p.phase;
      if (p.total > 0) {
        store.aiProgressCurrent.value = p.current;
        store.aiProgressTotal.value = p.total;
      }
      if (p.done) {
        if (p.error) {
          store.error.value = p.error;
          store.aiLoading.value = false;
          store.aiProgressPhase.value = '';
        } else {
          store.aiProgressPhase.value = '加载AI分析结果中';
          fetchAiDoneResults();
        }
        return;
      }
      store.timers.aiPollTimer = setTimeout(() => aiPollProgress(key), 1500);
    }).catch(() => {
      store.timers.aiPollTimer = setTimeout(() => aiPollProgress(key), 3000);
    });
  }

  async function fetchAiDoneResults() {
    try {
      const data = await api.fetchAiResult(store.euid.value);
      if (data && data.result) {
        store.aiResult.value = data.result;
      } else {
        store.error.value = 'AI分析结果格式异常';
      }
    } catch (e) {
      store.error.value = '获取AI分析结果失败';
    } finally {
      store.aiLoading.value = false;
      store.aiProgressPhase.value = '';
    }
  }

  async function aiAnalyze() {
    if (!store.euid.value.trim() || !store.stats.value) return;

    if (store.aiResult.value) {
      if (!confirm('该用户已有AI分析结果，是否重新分析？这将消耗 AI API 额度。')) return;
    }

    clearTimers();
    store.aiLoading.value = true;
    store.aiResult.value = null;
    store.error.value = '';
    store.aiProgressPhase.value = 'AI分析准备中';
    store.aiProgressCurrent.value = 0;
    store.aiProgressTotal.value = 0;

    try {
      const data = await api.startAiAnalysis(store.euid.value, store.userApiKeyParams());
      if (data.status === 'error') {
        store.error.value = data.error || 'AI分析启动失败';
        store.aiLoading.value = false;
        store.aiProgressPhase.value = '';
        return;
      }
      if (data.status === 'done') {
        store.aiResult.value = data.result;
        store.aiLoading.value = false;
        store.aiProgressPhase.value = '';
        return;
      }
      aiPollProgress(data.key);
    } catch (e) {
      store.error.value = '启动AI分析失败: ' + (e.message || '网络错误');
      store.aiLoading.value = false;
      store.aiProgressPhase.value = '';
    }
  }

  return { analyze, runSimilarity, aiAnalyze, fetchBasicResults, clearTimers };
}
