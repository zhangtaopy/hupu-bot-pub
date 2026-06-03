/**
 * MonitorTab — 分区舆论监控仪表盘
 */
import * as api from '../utils/api.js';

const { ref, reactive, computed, watch, nextTick, onUnmounted } = Vue;

export function setupMonitorTab(store) {
  // ── State ──
  const topicId = ref('cars'); // 默认汽车区
  const fetchDays = ref(1); // 默认抓取今天
  const monitorLoading = ref(false);
  const monitorError = ref('');
  const monitorStats = ref(null);
  const monitorPosts = ref([]);
  const monitorReplies = ref([]);
  const coveredDates = ref([]);
  const fetchProgress = reactive({ phase: '', current: 0, total: 0, done: false, error: null });
  const analyzeProgress = reactive({ phase: '', current: 0, total: 0, done: false, error: null });

  let fetchPollTimer = null;
  let analyzePollTimer = null;
  let sentimentChart = null;
  let dailyChart = null;
  let brandChart = null;
  let modelChart = null;

  // Quick-select topics
  const quickTopics = [
    { id: 'cars', name: '汽车区' },
    { id: 'topic-daily', name: '步行街主干道' },
    { id: 'lol', name: '英雄联盟' },
    { id: 'vote', name: '湿乎乎的话题' },
    { id: 'digital', name: '数码区' },
    { id: 'ent', name: '影视区' },
  ];

  // ── Computed ──
  const todayStats = computed(() => {
    if (!monitorStats.value || !monitorStats.value.today) return { posts: 0, replies: 0 };
    return monitorStats.value.today;
  });

  const latestSnapshot = computed(() => {
    if (!monitorStats.value || !monitorStats.value.snapshots || !monitorStats.value.snapshots.length)
      return null;
    return monitorStats.value.snapshots[0];
  });

  const sentimentData = computed(() => {
    const snap = latestSnapshot.value;
    if (!snap || !snap.sentiment_dist) return null;
    try {
      return typeof snap.sentiment_dist === 'string'
        ? JSON.parse(snap.sentiment_dist)
        : snap.sentiment_dist;
    } catch {
      return null;
    }
  });

  const aiJson = computed(() => {
    const snap = latestSnapshot.value;
    if (!snap || !snap.ai_raw_json) return null;
    try {
      return typeof snap.ai_raw_json === 'string'
        ? JSON.parse(snap.ai_raw_json)
        : snap.ai_raw_json;
    } catch { return null; }
  });

  const brandData = computed(() => {
    if (!aiJson.value || !aiJson.value.brand_analysis) return [];
    return aiJson.value.brand_analysis;
  });

  const modelData = computed(() => {
    if (!aiJson.value || !aiJson.value.key_models) return [];
    return aiJson.value.key_models;
  });

  const keywordsData = computed(() => {
    const snap = latestSnapshot.value;
    if (!snap || !snap.top_keywords) return [];
    try {
      const kw = typeof snap.top_keywords === 'string'
        ? JSON.parse(snap.top_keywords)
        : snap.top_keywords;
      return Array.isArray(kw) ? kw : [];
    } catch {
      return [];
    }
  });

  const dailyCounts = computed(() => {
    if (!monitorStats.value || !monitorStats.value.daily_counts) return [];
    return [...monitorStats.value.daily_counts].reverse(); // chronological order
  });

  // ── Actions ──

  async function loadExistingData() {
    if (!topicId.value.trim()) return;
    monitorError.value = '';
    monitorStats.value = null;
    monitorPosts.value = [];
    monitorReplies.value = [];
    await loadStats();
  }

  async function monitorFetchData() {
    if (!topicId.value.trim()) return;
    monitorError.value = '';
    monitorLoading.value = true;

    const params = new URLSearchParams({
      topic_id: topicId.value.trim(),
      days: String(fetchDays.value),
      replies_per_post: '10',
    });
    if (store.hasUserCookie()) params.append('cookie', store.userCookie.value.trim());

    try {
      const res = await fetch(`/api/monitor/fetch?${params}`, { method: 'POST' });
      const data = await res.json();
      if (data.status === 'started' || data.status === 'running') {
        // Start polling
        Object.assign(fetchProgress, { phase: '启动中', current: 0, total: 0, done: false, error: null });
        pollFetchProgress();
      } else if (data.error) {
        monitorError.value = data.error;
        monitorLoading.value = false;
      } else {
        monitorError.value = '未知响应: ' + JSON.stringify(data);
        monitorLoading.value = false;
      }
    } catch (e) {
      monitorError.value = e.message || '请求失败';
      monitorLoading.value = false;
    }
  }

  async function pollFetchProgress() {
    if (fetchPollTimer) clearTimeout(fetchPollTimer);
    try {
      const res = await fetch(`/api/monitor/fetch-progress?topic_id=${topicId.value.trim()}`);
      const data = await res.json();
      Object.assign(fetchProgress, data);
      if (data.done) {
        monitorLoading.value = false;
        if (data.error) {
          monitorError.value = data.error;
        } else {
          await loadStats();
        }
      } else {
        fetchPollTimer = setTimeout(pollFetchProgress, 1500);
      }
    } catch (e) {
      monitorLoading.value = false;
      monitorError.value = '轮询进度失败: ' + (e.message || '');
    }
  }

  async function loadStats() {
    try {
      const res = await fetch(`/api/monitor/stats?topic_id=${topicId.value.trim()}&days=${fetchDays.value}`);
      monitorStats.value = await res.json();
      await loadPosts();
      await loadReplies();
      await nextTick();
      renderSentimentChart();
      renderDailyChart();
      renderBrandChart();
      renderModelChart();
    } catch (e) {
      monitorError.value = '加载统计数据失败: ' + (e.message || '');
    }
  }

  async function loadPosts() {
    try {
      const res = await fetch(`/api/monitor/posts?topic_id=${topicId.value.trim()}&limit=20`);
      const data = await res.json();
      monitorPosts.value = data.posts || [];
    } catch { /* ignore */ }
  }

  async function loadReplies() {
    try {
      const res = await fetch(`/api/monitor/replies?topic_id=${topicId.value.trim()}&limit=20`);
      const data = await res.json();
      monitorReplies.value = data.replies || [];
    } catch { /* ignore */ }
  }

  async function startAnalyze() {
    monitorError.value = '';
    const params = new URLSearchParams({ topic_id: topicId.value.trim() });
    if (store.hasUserApiKey()) {
      params.append('api_key', store.userApiKey.value.trim());
      params.append('provider', store.userApiProvider.value);
    }

    try {
      const res = await fetch(`/api/monitor/analyze?${params}`, { method: 'POST' });
      const data = await res.json();
      if (data.status === 'started' || data.status === 'running') {
        Object.assign(analyzeProgress, { phase: '启动中', current: 0, total: 0, done: false, error: null });
        pollAnalyzeProgress();
      } else if (data.error) {
        monitorError.value = data.error;
      }
    } catch (e) {
      monitorError.value = e.message || '请求失败';
    }
  }

  async function pollAnalyzeProgress() {
    if (analyzePollTimer) clearTimeout(analyzePollTimer);
    try {
      const res = await fetch(`/api/monitor/analyze-progress?topic_id=${topicId.value.trim()}`);
      const data = await res.json();
      Object.assign(analyzeProgress, data);
      if (data.done) {
        if (data.error) {
          monitorError.value = data.error;
        } else {
          await loadStats();
        }
      } else {
        analyzePollTimer = setTimeout(pollAnalyzeProgress, 2000);
      }
    } catch (e) {
      monitorError.value = '轮询分析进度失败: ' + (e.message || '');
    }
  }

  // ── Chart rendering ──

  function renderSentimentChart() {
    const canvas = document.getElementById('monitor-sentiment-chart');
    if (!canvas) return;
    if (sentimentChart) sentimentChart.destroy();

    const sd = sentimentData.value;
    if (!sd) return;

    const ctx = canvas.getContext('2d');
    const total = (sd.positive || 0) + (sd.neutral || 0) + (sd.negative || 0);
    sentimentChart = new Chart(ctx, {
      type: 'doughnut',
      data: {
        labels: ['正面', '中性', '负面'],
        datasets: [{
          data: [sd.positive || 0, sd.neutral || 0, sd.negative || 0],
          backgroundColor: [
            'rgba(52,211,153,0.85)',
            'rgba(148,163,184,0.7)',
            'rgba(248,113,113,0.85)'
          ],
          borderColor: store.darkMode.value ? '#1f2937' : '#ffffff',
          borderWidth: 2,
          hoverOffset: 8,
        }]
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        cutout: '62%',
        plugins: {
          legend: {
            position: 'bottom',
            labels: {
              color: store.chartTextColor(),
              padding: 16,
              font: { size: 12 },
              usePointStyle: true,
              pointStyle: 'circle',
            }
          },
          tooltip: {
            backgroundColor: store.darkMode.value ? 'rgba(17,24,39,0.9)' : 'rgba(255,255,255,0.95)',
            titleColor: store.darkMode.value ? '#e2e8f0' : '#1e293b',
            bodyColor: store.darkMode.value ? '#cbd5e1' : '#475569',
            borderColor: store.darkMode.value ? 'rgba(75,85,99,0.5)' : 'rgba(226,232,240,0.8)',
            borderWidth: 1,
            padding: 10,
            cornerRadius: 8,
            callbacks: {
              label: (c) => {
                const val = c.raw || 0;
                const pct = total > 0 ? ((val / total) * 100).toFixed(1) : '0.0';
                return ` ${c.label}: ${val} (${pct}%)`;
              }
            }
          },
        },
      },
      plugins: [{
        id: 'centerText',
        beforeDraw: function(chart) {
          const { width, height, ctx } = chart;
          ctx.restore();
          const fontSize = (height / 160).toFixed(2);
          ctx.font = `bold ${fontSize}em sans-serif`;
          ctx.textBaseline = 'middle';
          ctx.fillStyle = store.darkMode.value ? '#e2e8f0' : '#1e293b';
          const text = String(total);
          const textX = Math.round((width - ctx.measureText(text).width) / 2);
          const textY = height / 2;
          ctx.fillText(text, textX, textY);
          ctx.save();
        }
      }]
    });
  }

  function renderBrandChart() {
    const canvas = document.getElementById('monitor-brand-chart');
    if (!canvas) return;
    if (brandChart) brandChart.destroy();
    const bd = brandData.value;
    if (!bd.length) return;

    const ctx = canvas.getContext('2d');
    brandChart = new Chart(ctx, {
      type: 'bar',
      data: {
        labels: bd.map(b => b.brand),
        datasets: [{
          label: '提及次数',
          data: bd.map(b => b.mention_count || 0),
          backgroundColor: bd.map(b => {
            switch (b.sentiment) {
              case '正面': return 'rgba(52,211,153,0.8)';
              case '负面': return 'rgba(248,113,113,0.8)';
              case '争议': return 'rgba(251,191,36,0.8)';
              default: return 'rgba(148,163,184,0.7)';
            }
          }),
          borderColor: bd.map(b => {
            switch (b.sentiment) {
              case '正面': return '#34d399';
              case '负面': return '#f87171';
              case '争议': return '#fbbf24';
              default: return '#94a3b8';
            }
          }),
          borderWidth: 1,
          borderRadius: 6,
          barThickness: 22,
          maxBarThickness: 28,
        }]
      },
      options: {
        indexAxis: 'y',
        responsive: true,
        maintainAspectRatio: false,
        plugins: {
          legend: { display: false },
          tooltip: {
            backgroundColor: store.darkMode.value ? 'rgba(17,24,39,0.9)' : 'rgba(255,255,255,0.95)',
            titleColor: store.darkMode.value ? '#e2e8f0' : '#1e293b',
            bodyColor: store.darkMode.value ? '#cbd5e1' : '#475569',
            borderColor: store.darkMode.value ? 'rgba(75,85,99,0.5)' : 'rgba(226,232,240,0.8)',
            borderWidth: 1,
            padding: 10,
            cornerRadius: 8,
          },
        },
        scales: {
          x: {
            ticks: { color: store.chartTextColor(), font: { size: 10 } },
            grid: { color: store.chartGridColor() },
          },
          y: {
            ticks: { color: store.chartTextColor(), font: { size: 11 } },
            grid: { display: false },
          },
        },
      },
    });
  }

  function findPct(obj, kind) {
    const patterns = kind === 'pos'
      ? ['positive', 'good', 'pos', 'like']
      : ['negative', 'bad', 'neg'];
    for (const k of Object.keys(obj || {})) {
      const kl = k.toLowerCase();
      const hasPattern = patterns.some(p => kl.includes(p));
      const hasPct = kl.includes('pct') || kl.includes('percent') || kl.includes('ratio') || kl.includes('review');
      if (hasPattern && hasPct) return obj[k];
    }
    return 0;
  }

  function renderModelChart() {
    const canvas = document.getElementById('monitor-model-chart');
    if (!canvas) return;
    if (modelChart) modelChart.destroy();
    const md = modelData.value;
    if (!md.length) return;

    const ctx = canvas.getContext('2d');
    modelChart = new Chart(ctx, {
      type: 'bar',
      data: {
        labels: md.map(m => m.model || m.name || ''),
        datasets: [
          {
            label: '好评',
            data: md.map(m => {
              let v = findPct(m, 'pos');
              if (typeof v === 'string') v = parseFloat(v) || 0;
              return v > 1 ? v : v * 100;
            }),
            backgroundColor: 'rgba(52,211,153,0.8)',
            borderColor: '#34d399',
            borderWidth: 1,
            borderRadius: { topLeft: 5, bottomLeft: 5 },
            borderSkipped: false,
            barThickness: 20,
            maxBarThickness: 26,
          },
          {
            label: '差评',
            data: md.map(m => {
              let v = findPct(m, 'neg');
              if (typeof v === 'string') v = parseFloat(v) || 0;
              return v > 1 ? v : v * 100;
            }),
            backgroundColor: 'rgba(248,113,113,0.8)',
            borderColor: '#f87171',
            borderWidth: 1,
            borderRadius: { topRight: 5, bottomRight: 5 },
            borderSkipped: false,
            barThickness: 20,
            maxBarThickness: 26,
          },
        ]
      },
      options: {
        indexAxis: 'y',
        responsive: true,
        maintainAspectRatio: false,
        plugins: {
          legend: {
            position: 'top',
            labels: { color: store.chartTextColor(), font: { size: 10 }, padding: 10, usePointStyle: true },
          },
          tooltip: {
            backgroundColor: store.darkMode.value ? 'rgba(17,24,39,0.9)' : 'rgba(255,255,255,0.95)',
            titleColor: store.darkMode.value ? '#e2e8f0' : '#1e293b',
            bodyColor: store.darkMode.value ? '#cbd5e1' : '#475569',
            borderColor: store.darkMode.value ? 'rgba(75,85,99,0.5)' : 'rgba(226,232,240,0.8)',
            borderWidth: 1,
            padding: 10,
            cornerRadius: 8,
            callbacks: {
              label: (c) => ` ${c.dataset.label}: ${c.raw.toFixed ? c.raw.toFixed(1) : c.raw}%`
            }
          },
        },
        scales: {
          x: {
            stacked: true,
            max: 100,
            ticks: { color: store.chartTextColor(), font: { size: 10 }, callback: v => v + '%' },
            grid: { color: store.chartGridColor() },
          },
          y: {
            stacked: true,
            ticks: { color: store.chartTextColor(), font: { size: 11 } },
            grid: { display: false },
          },
        },
      },
    });
  }

  function renderDailyChart() {
    const canvas = document.getElementById('monitor-daily-chart');
    if (!canvas) return;
    if (dailyChart) dailyChart.destroy();

    const dc = dailyCounts.value;
    if (!dc.length) return;

    const ctx = canvas.getContext('2d');
    const gradient = ctx.createLinearGradient(0, 0, 0, 220);
    gradient.addColorStop(0, 'rgba(129, 140, 248, 0.35)');
    gradient.addColorStop(1, 'rgba(129, 140, 248, 0.02)');

    dailyChart = new Chart(ctx, {
      type: 'line',
      data: {
        labels: dc.map(d => d.date),
        datasets: [{
          label: '每日帖子数',
          data: dc.map(d => d.count),
          borderColor: '#818cf8',
          backgroundColor: gradient,
          fill: true,
          tension: 0.4,
          pointRadius: 5,
          pointHoverRadius: 7,
          pointBackgroundColor: '#818cf8',
          pointBorderColor: store.darkMode.value ? '#1f2937' : '#ffffff',
          pointBorderWidth: 2,
          borderWidth: 2.5,
        }]
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        interaction: { mode: 'index', intersect: false },
        plugins: {
          legend: {
            labels: { color: store.chartTextColor(), font: { size: 11 }, usePointStyle: true }
          },
          tooltip: {
            backgroundColor: store.darkMode.value ? 'rgba(17,24,39,0.9)' : 'rgba(255,255,255,0.95)',
            titleColor: store.darkMode.value ? '#e2e8f0' : '#1e293b',
            bodyColor: store.darkMode.value ? '#cbd5e1' : '#475569',
            borderColor: store.darkMode.value ? 'rgba(75,85,99,0.5)' : 'rgba(226,232,240,0.8)',
            borderWidth: 1,
            padding: 10,
            cornerRadius: 8,
          },
        },
        scales: {
          x: {
            ticks: { color: store.chartTextColor(), font: { size: 10 } },
            grid: { color: store.chartGridColor() },
          },
          y: {
            ticks: { color: store.chartTextColor(), font: { size: 10 }, stepSize: 1 },
            grid: { color: store.chartGridColor() },
            beginAtZero: true,
          },
        },
      },
    });
  }

  // ── Watch topicId → reload ──
  watch(topicId, () => {
    monitorStats.value = null;
    monitorPosts.value = [];
    monitorReplies.value = [];
    monitorError.value = '';
    Object.assign(fetchProgress, { phase: '', current: 0, total: 0, done: false, error: null });
    Object.assign(analyzeProgress, { phase: '', current: 0, total: 0, done: false, error: null });
  });

  // ── Watch darkMode → re-render charts ──
  watch(store.darkMode, async () => {
    await nextTick();
    if (monitorStats.value) {
      renderSentimentChart();
      renderDailyChart();
      renderBrandChart();
      renderModelChart();
    }
  });

  // ── Cleanup ──
  onUnmounted(() => {
    if (fetchPollTimer) clearTimeout(fetchPollTimer);
    if (analyzePollTimer) clearTimeout(analyzePollTimer);
    if (sentimentChart) sentimentChart.destroy();
    if (dailyChart) dailyChart.destroy();
    if (brandChart) brandChart.destroy();
    if (modelChart) modelChart.destroy();
  });

  // Format helpers
  function fmtTime(ts) {
    if (!ts) return '';
    const d = new Date(ts * 1000);
    return d.toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
  }

  function truncate(s, n) {
    if (!s) return '';
    return s.length > n ? s.slice(0, n) + '...' : s;
  }

  return {
    topicId, fetchDays, monitorLoading, monitorError, monitorStats, monitorPosts, monitorReplies,
    coveredDates, fetchProgress, analyzeProgress,
    quickTopics,
    todayStats, latestSnapshot, sentimentData, aiJson, brandData, modelData, keywordsData, dailyCounts,
    loadExistingData, monitorFetchData, loadStats, startAnalyze, pollFetchProgress, pollAnalyzeProgress,
    renderSentimentChart, renderDailyChart,
    fmtTime, truncate,
  };
}
