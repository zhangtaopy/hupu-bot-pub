/**
 * QA Tab — streaming Q&A with AI.
 */
import * as api from '../utils/api.js';

export function setupQaTab(store) {
  async function askQuestion() {
    if (!store.euid.value.trim() || !store.qaQuestion.value.trim()) return;

    const question = store.qaQuestion.value.trim();
    store.qaQuestion.value = '';
    store.qaError.value = '';
    store.qaLoading.value = true;

    const now = new Date();
    const time = now.getHours().toString().padStart(2, '0') + ':' + now.getMinutes().toString().padStart(2, '0');

    const entry = Vue.reactive({ question, answer: '', time, error: null, thinking: true, rounds: [], showDetail: false, promptTokens: 0, completionTokens: 0 });
    store.qaHistory.value.push(entry);

    const history = store.qaHistory.value
      .filter(m => !m.thinking && !m.error && m.answer)
      .slice(-3)
      .map(m => ({ question: m.question, answer: m.answer }));

    try {
      const res = await fetch('/api/qa/ask', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          euid: store.euid.value.trim(),
          question,
          history,
          api_key: store.userApiKey.value.trim() || undefined,
          provider: store.userApiKey.value.trim() ? store.userApiProvider.value : undefined,
        }),
      });

      if (!res.ok) {
        const text = await res.text();
        throw new Error(text || `请求失败 (HTTP ${res.status})`);
      }

      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let buffer = '';

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;

        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split('\n');
        buffer = lines.pop() || '';

        for (const line of lines) {
          if (!line.trim()) continue;
          try {
            const event = JSON.parse(line);
            if (event.type === 'round') {
              entry.rounds.push({
                round: event.round, action: event.action,
                keywords: event.keywords || [],
                search_tables: event.search_tables || [],
                reply_count: event.reply_count || 0,
                post_count: event.post_count || 0,
                reasoning: event.reasoning || '',
                tool_calls: event.tool_calls || null,
              });
              await Vue.nextTick();
            } else if (event.type === 'tool_call') {
              const currentRound = entry.rounds.find(r => r.round === event.round);
              if (currentRound) {
                if (!currentRound.tool_calls) currentRound.tool_calls = [];
                currentRound.tool_calls.push({
                  tool_name: event.tool_name,
                  args_summary: event.args_summary,
                  result_summary: event.result_summary,
                });
              }
              await Vue.nextTick();
            } else if (event.type === 'answer') {
              entry.thinking = false;
              entry.answer = event.answer || '';
              entry.promptDetail = event.prompt_detail || '';
              entry.promptTokens = event.prompt_tokens || 0;
              entry.completionTokens = event.completion_tokens || 0;
              if (event.username) store.qaUsername.value = event.username;
            } else if (event.type === 'error') {
              entry.thinking = false;
              entry.error = event.error || '未知错误';
              store.qaError.value = event.error || '未知错误';
            }
          } catch (e) {
            console.warn('解析流事件失败:', line, e);
          }
        }
      }

      // Non-blocking stats query
      try {
        const statsRes = await api.fetchStats(store.euid.value);
        if (statsRes) store.qaReplyCount.value = statsRes.total_replies || 0;
        const postsJson = await api.fetchPosts(store.euid.value, 1, 0);
        if (postsJson) store.qaPostCount.value = postsJson.total || 0;
      } catch (e) { /* ignore */ }
    } catch (e) {
      entry.thinking = false;
      entry.error = e.message || '问答请求失败';
      store.qaError.value = e.message || '问答请求失败';
    } finally {
      store.qaLoading.value = false;
    }
  }

  function togglePromptDetail(idx) {
    const msg = store.qaHistory.value[idx];
    if (msg) msg.showDetail = !msg.showDetail;
  }

  return { askQuestion, togglePromptDetail };
}
