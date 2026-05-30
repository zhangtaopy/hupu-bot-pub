/**
 * Chart rendering functions — Chart.js + WordCloud.
 * These are called after data loads, NOT exported to Vue template.
 */

export function setupCharts(store) {
  const { chartInstances, chartTextColor, chartGridColor, generateColors, darkMode } = store;

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
            borderColor: 'rgb(99, 102, 241)',
            backgroundColor: 'rgba(99, 102, 241, 0.08)',
            fill: true, tension: 0.4, pointRadius: 3,
            pointBackgroundColor: 'rgb(99, 102, 241)', borderWidth: 2,
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
    const colors = ['#6366F1','#EC4899','#10B981','#F59E0B','#8B5CF6','#EF4444','#06B6D4','#F97316','#3B82F6','#14B8A6'];

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
          backgroundColor: darkMode.value ? '#1e293b' : '#ffffff',
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
              if (i >= 7 && i <= 11) return '#F59E0B';
              if (i >= 12 && i <= 13) return '#10B981';
              if (i >= 20 || i <= 5) return '#8B5CF6';
              return '#6366F1';
            }),
            borderRadius: 4,
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
            backgroundColor: ['#6366F1','#6366F1','#6366F1','#6366F1','#6366F1','#10B981','#EF4444'],
            borderRadius: 4,
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
          datasets: [{ data: buckets, backgroundColor: ['#06B6D4','#6366F1','#F59E0B','#F97316','#EF4444'] }]
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
