/**
 * Main entry point — imports store + components, creates and mounts the Vue app.
 */
import { createStore } from './store.js';
import { setupCharts } from './components/Charts.js';
import { setupHeader } from './components/Header.js';
import { setupConfigOverlay } from './components/ConfigOverlay.js';
import { setupFetchSection } from './components/FetchSection.js';
import { setupRepliesTab } from './components/RepliesTab.js';
import { setupPostsTab } from './components/PostsTab.js';
import { setupQaTab } from './components/QaTab.js';
import { setupMonitorTab } from './components/MonitorTab.js';
import * as api from './utils/api.js';
import { fmtTokens, renderMarkdown } from './utils/helpers.js';

const { createApp, watch, nextTick, onMounted } = Vue;

createApp({
  setup() {
    // ── Create store (shared reactive state) ──
    const store = createStore();

    // ── Setup components ──
    const charts = setupCharts(store);
    const header = setupHeader(store);
    const configOverlay = setupConfigOverlay(store);
    const fetchSection = setupFetchSection(store);
    const replies = setupRepliesTab(store, charts, { api });
    const posts = setupPostsTab(store, charts);
    const qa = setupQaTab(store);
    const monitor = setupMonitorTab(store);

    // ── Watch darkMode → re-render charts ──
    watch(store.darkMode, async () => {
      await nextTick();
      if (store.stats.value) {
        charts.renderCharts();
        charts.renderWordCloud();
        charts.renderDetailedCharts();
      }
      if (store.postsData.value.length) {
        charts.renderPostTopicChart();
      }
      if (monitor.monitorStats.value) {
        monitor.renderSentimentChart();
        monitor.renderDailyChart();
        monitor.renderBrandChart();
        monitor.renderModelChart();
      }
    });

    // ── Watch activeTab → render charts when switching ──
    watch(store.activeTab, async (newTab) => {
      await nextTick();
      if (newTab === 'replies' && store.stats.value) {
        charts.renderCharts();
        charts.renderWordCloud();
        charts.renderDetailedCharts();
      } else if (newTab === 'posts' && store.postsData.value.length) {
        charts.renderPostTopicChart();
      } else if (newTab === 'monitor' && monitor.monitorStats.value) {
        monitor.renderSentimentChart();
        monitor.renderDailyChart();
        monitor.renderBrandChart();
        monitor.renderModelChart();
      }
    });

    // ── Watch euid → clear QA history ──
    watch(store.euid, () => {
      store.hasSimilarityResults.value = false;
      store.qaHistory.value = [];
      store.qaUsername.value = '';
      store.qaReplyCount.value = 0;
      store.qaPostCount.value = 0;
      store.qaError.value = '';
    });

    // ── Watch threshold → re-fetch similarity ──
    watch(store.threshold, () => {
      if (!store.euid.value.trim() || !store.stats.value || !store.hasSimilarityResults.value) return;
      clearTimeout(store.timers.debounceTimer);
      if (store.timers.pollTimer) clearTimeout(store.timers.pollTimer);
      store.debounceTimer = setTimeout(async () => {
        try {
          const simData = await api.fetchSimilarity(store.euid.value, store.threshold.value);
          if (simData && simData.status === 'done') {
            store.groups.value = simData.groups;
          } else {
            replies.runSimilarity();
          }
        } catch (e) { /* ignore */ }
      }, 300);
    });

    // ── Export image ──
    async function exportImage() {
      if (typeof html2canvas === 'undefined') {
        store.error.value = '截图库加载失败，请刷新页面重试';
        return;
      }
      const el = document.getElementById('capture-area');
      if (!el || el.offsetHeight === 0) {
        store.error.value = '暂无内容可导出';
        return;
      }
      store.groups.value.forEach(g => { store.expandedGroups[g.group_id] = true; });
      await nextTick();
      await new Promise(r => setTimeout(r, 100));
      try {
        const canvas = await html2canvas(el, {
          scale: 2, useCORS: true, backgroundColor: '#f8fafc', logging: false,
        });
        canvas.toBlob(function(blob) {
          const url = URL.createObjectURL(blob);
          const link = document.createElement('a');
          link.download = `hupu-analysis-${store.euid.value || 'user'}.png`;
          link.href = url;
          link.click();
          URL.revokeObjectURL(url);
        }, 'image/png');
      } catch (e) {
        store.error.value = '导出图片失败: ' + (e.message || '未知错误');
      }
    }

    // ── Config helpers ──
    async function saveConfig() {
      store.configSaving.value = true;
      store.configError.value = '';
      try {
        const payload = store.needsAiKey.value
          ? { cookie: '', provider: store.configProvider.value, api_key: store.configAiKey.value, model: store.configModel.value }
          : { cookie: store.configCookie.value, provider: store.configProvider.value, api_key: store.configAiKey.value, model: store.configModel.value };
        await api.saveConfig(payload);
        window.location.reload();
      } catch (e) {
        store.configError.value = e.message || '保存失败';
      } finally {
        store.configSaving.value = false;
      }
    }

    function skipAiKeySetup() {
      store.needsAiKey.value = false;
    }

    async function checkConfigStatus() {
      try {
        const data = await api.fetchConfigStatus();
        if (data) {
          if (data.deploy_mode) {
            store.needsConfig.value = false;
            store.needsAiKey.value = false;
            return;
          }
          store.needsConfig.value = !data.configured && !store.hasLocalCredentials();
          store.needsAiKey.value = data.configured && !data.has_api_key && !store.hasUserApiKey();
        }
      } catch (e) {
        store.needsConfig.value = !store.hasLocalCredentials();
      }
    }

    // ── Fetch euids on mount ──
    async function fetchEuids() {
      const list = await api.fetchEuidsList();
      store.euidsList.value = list;
    }

    onMounted(async () => {
      await checkConfigStatus();
      if (!store.needsConfig.value) {
        fetchEuids();
      }
    });

    // ── Return all bindings to template ──
    return {
      ...store,
      ...header,
      ...replies,
      ...posts,
      ...qa,
      ...monitor,
      ...fetchSection,
      saveConfig, skipAiKeySetup, checkConfigStatus,
      fetchEuids, exportImage,
      fmtTokens, renderMarkdown,
    };
  }
}).mount('#app');
