/**
 * Chart rendering functions — Chart.js + WordCloud.
 * These are called after data loads, NOT exported to Vue template.
 */

import { generateColors } from '../utils/helpers.js';

export function setupCharts(store) {
  const { chartInstances, chartTextColor, chartGridColor, darkMode } = store;

  function renderCharts() {
    if (!store.stats.value) return;

    const topicCtx = document.getElementById('topicChart');
    if (topicCtx) {
      if (chartInstances.topicChart) chartInstances.topicChart.destroy();
      const td = store.stats.value.topic_distribution;
      const labels = Object.keys(td);
      const data = Object.values(td);
      chartInstances.topicChart = new Chart(topicCtx, {
        type: 'doughnut',
        data: { labels, datasets: [{ data, backgroundColor: generateColors(labels.length) }] },
        options: {
          responsive: true,
          plugins: {
            legend: { position: 'right', labels: { color: chartTextColor(), boxWidth: 12, font: { size: 11 } } }
          }
        }
      });
    }

    const timeCtx = document.getElementById('timeChart');
    if (timeCtx) {
      if (chartInstances.timeChart) chartInstances.timeChart.destroy();
      const td = store.stats.value.time_distribution;
      const labels = Object.keys(td);
      const data = Object.values(td);
      chartInstances.timeChart = new Chart(timeCtx, {
        type: 'line',
        data: {
          labels,
          datasets: [{
            label: '回帖数', data,
            borderColor: '#007AFF',
            backgroundColor: 'rgba(0, 122, 255, 0.1)',
            fill: true, tension: 0.4, pointRadius: 3,
            pointBackgroundColor: '#007AFF', borderWidth: 2,
          }]
        },
        options: {
          responsive: true,
          scales: {
            y: { beginAtZero: true, ticks: { stepSize: 1, color: chartTextColor() }, grid: { color: chartGridColor() } },
            x: { ticks: { font: { size: 10 }, color: chartTextColor() }, grid: { color: chartGridColor() } }
          },
          plugins: { legend: { display: false } }
        }
      });
    }
  }

  function renderWordCloud() {
    if (!store.wordCloudWords.value.length) return;
    const canvas = document.getElementById('wordCloudCanvas');
    if (!canvas) return;

    const list = store.wordCloudWords.value.map(w => [w.text, w.count]);
    const maxCount = Math.max(...store.wordCloudWords.value.map(w => w.count));
    const minCount = Math.min(...store.wordCloudWords.value.map(w => w.count));
    const weightFactor = canvas.offsetWidth / 60;
    const colors = ['#007AFF','#34C759','#FF9500','#FF3B30','#AF52DE','#5AC8FA','#FF2D55','#5856D6','#FFCC00','#00C7BE'];

    canvas.width = canvas.offsetWidth;
    canvas.height = 350;

    if (window.WordCloud) {
      try {
        window.WordCloud(canvas, {
          list, gridSize: 10,
          weightFactor: w => weightFactor * (0.5 + 1.5 * (w - minCount) / (maxCount - minCount)),
          fontFamily: 'PingFang SC, Microsoft YaHei, sans-serif',
          color: () => colors[Math.floor(Math.random() * colors.length)],
          rotateRatio: 0.3, rotationSteps: 2,
          backgroundColor: darkMode.value ? '#1c1c1e' : '#ffffff',
          weightMode: 'size', shape: 'circle', ellipticity: 0.6,
        });
      } catch (e) { console.warn('WordCloud render failed:', e); }
    }
  }

  function renderDetailedCharts() {
    if (!store.detailedAnalysis.value) return;

    const hourCtx = document.getElementById('hourChart');
    if (hourCtx) {
      if (chartInstances.hourChart) chartInstances.hourChart.destroy();
      const hourLabels = Array.from({length: 24}, (_, i) => i + '时');
      const hourData = store.detailedAnalysis.value.hour_distribution;
      chartInstances.hourChart = new Chart(hourCtx, {
        type: 'bar',
        data: {
          labels: hourLabels,
          datasets: [{
            label: '回帖数', data: hourData,
            backgroundColor: hourData.map((v, i) => {
              if (i >= 7 && i <= 11) return '#FF9500';
              if (i >= 12 && i <= 13) return '#34C759';
              if (i >= 20 || i <= 5) return '#AF52DE';
              return '#007AFF';
            }),
            borderRadius: 5,
          }]
        },
        options: {
          responsive: true,
          scales: {
            y: { beginAtZero: true, ticks: { stepSize: 1, font: { size: 10 }, color: chartTextColor() }, grid: { color: chartGridColor() } },
            x: { ticks: { font: { size: 9 }, maxTicksLimit: 24, color: chartTextColor() }, grid: { color: chartGridColor() } }
          },
          plugins: { legend: { display: false } }
        }
      });
    }

    const wdCtx = document.getElementById('weekdayChart');
    if (wdCtx) {
      if (chartInstances.weekdayChart) chartInstances.weekdayChart.destroy();
      chartInstances.weekdayChart = new Chart(wdCtx, {
        type: 'bar',
        data: {
          labels: ['周一','周二','周三','周四','周五','周六','周日'],
          datasets: [{
            label: '回帖数', data: store.detailedAnalysis.value.weekday_distribution,
            backgroundColor: ['#007AFF','#007AFF','#007AFF','#007AFF','#007AFF','#34C759','#FF3B30'],
            borderRadius: 5,
          }]
        },
        options: {
          responsive: true,
          scales: {
            y: { beginAtZero: true, ticks: { stepSize: 1, font: { size: 10 }, color: chartTextColor() }, grid: { color: chartGridColor() } },
            x: { ticks: { font: { size: 11 }, color: chartTextColor() }, grid: { color: chartGridColor() } }
          },
          plugins: { legend: { display: false } }
        }
      });
    }

    const lenCtx = document.getElementById('lengthChart');
    if (lenCtx) {
      if (chartInstances.lengthChart) chartInstances.lengthChart.destroy();
      const buckets = store.detailedAnalysis.value.reply_length_buckets;
      chartInstances.lengthChart = new Chart(lenCtx, {
        type: 'doughnut',
        data: {
          labels: ['1-10字','11-50字','51-100字','101-200字','200+字'],
          datasets: [{ data: buckets, backgroundColor: ['#5AC8FA','#007AFF','#FF9500','#FF9F0A','#FF3B30'] }]
        },
        options: {
          responsive: true,
          plugins: {
            legend: { position: 'right', labels: { color: chartTextColor(), boxWidth: 12, font: { size: 10 } } }
          }
        }
      });
    }
  }

  function renderPostTopicChart() {
    if (!store.postsData.value.length) return;
    const canvas = document.getElementById('postTopicChart');
    if (!canvas) return;
    if (chartInstances.postTopicChart) chartInstances.postTopicChart.destroy();

    const dist = {};
    store.postsData.value.forEach(p => {
      const topic = p.topic_name || p.forum_name || '未知';
      dist[topic] = (dist[topic] || 0) + 1;
    });
    const labels = Object.keys(dist);
    const data = Object.values(dist);
    const colors = generateColors(labels.length);

    chartInstances.postTopicChart = new Chart(canvas, {
      type: 'doughnut',
      data: { labels, datasets: [{ data, backgroundColor: colors }] },
      options: {
        responsive: true,
        plugins: { legend: { position: 'right', labels: { color: chartTextColor(), boxWidth: 12, font: { size: 11 } } } }
      }
    });
  }

  function destroyAll() {
    Object.values(chartInstances).forEach(chart => { if (chart) chart.destroy(); });
  }

  return { renderCharts, renderWordCloud, renderDetailedCharts, renderPostTopicChart, destroyAll };
}
