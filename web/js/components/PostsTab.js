/**
 * Posts Tab — user posts list, AI post analysis.
 */
import * as api from '../utils/api.js';

export function setupPostsTab(store, charts) {
  async function loadPosts() {
    if (!store.euid.value.trim()) return;
    store.postLoading.value = true;
    store.postsData.value = [];
    store.aiPostResult.value = null;
    store.error.value = '';

    try {
      const [userData, postsJson] = await Promise.all([
        api.fetchUser(store.euid.value),
        api.fetchPosts(store.euid.value, 200, 0),
      ]);
      store.userInfo.value = userData;
      store.postsData.value = postsJson.posts || [];
      store.displayedEuid.value = store.euid.value;

      store.totalReplies.value = store.postsData.value.reduce((s, p) => s + p.replies, 0);
      store.totalVisits.value = store.postsData.value.reduce((s, p) => s + p.visits, 0);
      store.totalLights.value = store.postsData.value.reduce((s, p) => s + p.lights, 0);
      store.videoCount.value = store.postsData.value.filter(p => p.has_video).length;

      await Vue.nextTick();
      charts.renderPostTopicChart();

      const aiData = await api.fetchAiPostResult(store.euid.value);
      if (aiData && aiData.status === 'done' && aiData.result) {
        store.aiPostResult.value = aiData.result;
      }
    } catch (e) {
      store.error.value = e.message || '获取发帖数据失败';
    } finally {
      store.postLoading.value = false;
    }
  }

  function aiPostPollProgress() {
    api.fetchAiPostProgress(store.euid.value).then(p => {
      if (p.phase && p.phase !== 'idle') store.aiPostProgressPhase.value = p.phase;
      if (p.total > 0) {
        store.aiPostProgressCurrent.value = p.current;
        store.aiPostProgressTotal.value = p.total;
      }
      if (p.done) {
        if (p.error) {
          store.error.value = p.error;
          store.aiPostLoading.value = false;
          store.aiPostProgressPhase.value = '';
        } else {
          store.aiPostProgressPhase.value = '加载AI分析结果中';
          fetchAiPostDoneResults();
        }
        return;
      }
      store.timers.postAiPollTimer = setTimeout(aiPostPollProgress, 1500);
    }).catch(() => {
      store.timers.postAiPollTimer = setTimeout(aiPostPollProgress, 3000);
    });
  }

  async function fetchAiPostDoneResults() {
    try {
      const data = await api.fetchAiPostResult(store.euid.value);
      if (data && data.result) {
        store.aiPostResult.value = data.result;
      } else {
        store.error.value = 'AI发帖分析结果格式异常';
      }
    } catch (e) {
      store.error.value = '获取AI发帖分析结果失败';
    } finally {
      store.aiPostLoading.value = false;
      store.aiPostProgressPhase.value = '';
    }
  }

  async function aiPostAnalyze() {
    if (!store.euid.value.trim() || !store.postsData.value.length) return;

    if (store.aiPostResult.value) {
      if (!confirm('该用户已有AI发帖分析结果，是否重新分析？这将消耗 AI API 额度。')) return;
    }

    if (store.timers.postAiPollTimer) clearTimeout(store.timers.postAiPollTimer);
    store.aiPostLoading.value = true;
    store.aiPostResult.value = null;
    store.error.value = '';
    store.aiPostProgressPhase.value = 'AI发帖分析准备中';
    store.aiPostProgressCurrent.value = 0;
    store.aiPostProgressTotal.value = 0;

    try {
      const data = await api.startAiPostAnalysis(store.euid.value, store.userApiKeyParams());
      if (data.status === 'error') {
        store.error.value = data.error || 'AI发帖分析启动失败';
        store.aiPostLoading.value = false;
        store.aiPostProgressPhase.value = '';
        return;
      }
      aiPostPollProgress();
    } catch (e) {
      store.error.value = '启动AI发帖分析失败: ' + (e.message || '网络错误');
      store.aiPostLoading.value = false;
      store.aiPostProgressPhase.value = '';
    }
  }

  return { loadPosts, aiPostAnalyze };
}
