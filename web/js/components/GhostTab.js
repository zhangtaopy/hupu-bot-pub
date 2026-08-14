/**
 * Ghost Tab — 查成分（成分卡）+ 魂穿（嘴替 / 对线模拟）。
 * 成分卡：一次性 POST 同步返回卡片 JSON。
 * 魂穿：SSE/NDJSON 流式对话，AI 模仿目标用户的说话风格。
 */
export function setupGhostTab(store) {
  // ── 查成分 ──
  async function ghostProfile() {
    const euid = store.euid.value.trim();
    if (!euid || store.profileLoading.value) return;

    store.profileLoading.value = true;
    store.profileError.value = '';
    store.profileStage.value = '准备中…';
    try {
      const params = new URLSearchParams({ euid });
      // 已有卡片时点按钮 = 重新生成，强制刷新缓存
      if (store.profileCard.value) params.set('refresh', 'true');
      if (store.userApiKey.value.trim()) {
        params.set('api_key', store.userApiKey.value.trim());
        params.set('provider', store.userApiProvider.value);
      }
      const res = await fetch('/api/ghost/profile?' + params.toString(), { method: 'POST' });

      if (!res.ok) {
        const text = await res.text();
        throw new Error(text || `请求失败 (HTTP ${res.status})`);
      }

      // NDJSON 流式读取：stage 事件更新进度，done 事件拿到卡片
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
            if (event.type === 'stage') {
              store.profileStage.value = event.stage || '';
            } else if (event.type === 'done') {
              store.profileCard.value = event.result;
              store.profileCached.value = !!event.cached;
            } else if (event.type === 'error') {
              store.profileError.value = event.error || '生成失败';
            }
          } catch (e) { /* 忽略解析失败的行 */ }
        }
        await Vue.nextTick();
      }
    } catch (e) {
      store.profileError.value = e.message || '生成失败';
    } finally {
      store.profileLoading.value = false;
      store.profileStage.value = '';
    }
  }

  // ── 魂穿 ──
  function setGhostMode(mode) {
    if (store.ghostLoading.value) return;
    store.ghostMode.value = mode;
    store.ghostInput.value = '';
    store.ghostError.value = '';
  }

  async function ghostSend() {
    const euid = store.euid.value.trim();
    const content = store.ghostInput.value.trim();
    if (!euid || !content || store.ghostLoading.value) return;

    store.ghostInput.value = '';
    store.ghostError.value = '';

    const now = new Date();
    const time = now.getHours().toString().padStart(2, '0') + ':' + now.getMinutes().toString().padStart(2, '0');
    const entry = Vue.reactive({
      question: content,
      answer: '',
      time,
      error: null,
      thinking: true,
      username: store.ghostUsername.value,
    });
    store.ghostHistory.value.push(entry);

    // 最近几轮作为对话上下文（跳过 thinking/error 未完成的）
    const history = store.ghostHistory.value
      .filter(m => !m.thinking && !m.error && m.answer)
      .slice(-6)
      .map(m => ({ question: m.question, answer: m.answer }));

    try {
      const res = await fetch('/api/ghost/chat', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          euid,
          mode: store.ghostMode.value,
          content,
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
            if (event.type === 'start') {
              store.ghostUsername.value = event.username || store.ghostUsername.value;
              entry.username = store.ghostUsername.value;
            } else if (event.type === 'answer') {
              entry.answer = event.answer || '';
              entry.thinking = false;
            } else if (event.type === 'error') {
              entry.error = event.error || '未知错误';
              entry.thinking = false;
            }
          } catch (e) { /* 忽略解析失败的行 */ }
        }
        await Vue.nextTick();
      }
      entry.thinking = false;
    } catch (e) {
      entry.error = e.message || '请求失败';
      entry.thinking = false;
    }
  }

  return { ghostProfile, setGhostMode, ghostSend };
}
