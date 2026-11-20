/** Evidence-backed topic history. Query changes invalidate in-flight responses. */
export function setupHistoryTab(store) {
  const { ref, computed, watch, onUnmounted } = Vue;
  const historyTopic = ref('');
  const historyAliases = ref('');
  const historyStart = ref('');
  const historyEnd = ref('');
  const historyData = ref(null);
  const historyLoading = ref(false);
  const historyStage = ref('');
  const historyError = ref('');
  const historyEvidence = ref(null);
  let controller;
  let generation = 0;

  const historyReport = computed(() => historyData.value?.report);
  const historyClaims = computed(() => historyReport.value?.analysis.claims || []);
  const historyLegacy = computed(() => !!historyReport.value && historyReport.value.report_version !== 2);
  const historyHighlights = computed(() => {
    const curated = historyReport.value?.analysis.highlights || [];
    const entries = curated.length ? curated : historyClaims.value.slice().sort((a, b) =>
      (historySource(b.evidence_id)?.light_count || 0) - (historySource(a.evidence_id)?.light_count || 0)
    ).map(c => ({ claim_id: c.id, comment: c.stance }));
    const seen = new Set();
    return entries.flatMap(h => {
      const claim = historyClaim(h.claim_id);
      const source = claim && historySource(claim.evidence_id);
      if (!source || seen.has(source.id)) return [];
      seen.add(source.id);
      return [{ ...h, claim, source }];
    }).slice(0, 6);
  });
  function historyClaim(id) { return historyClaims.value.find(c => c.id === id); }
  function historySource(id) { return historyReport.value?.evidence.find(e => e.id === id); }
  function historyShowClaim(id) {
    const claim = historyClaim(id);
    if (claim) historyEvidence.value = { ...historySource(claim.evidence_id), excerpt: claim.excerpt };
  }
  const historyEvidenceParts = computed(() => {
    const e = historyEvidence.value;
    if (!e) return { before: '', excerpt: '', after: '' };
    const index = e.content.indexOf(e.excerpt);
    return { before: e.content.slice(0, index), excerpt: e.excerpt, after: e.content.slice(index + e.excerpt.length) };
  });
  function reset() {
    generation++;
    controller?.abort();
    historyLoading.value = false;
    historyData.value = null;
    historyError.value = '';
    historyEvidence.value = null;
  }
  watch([store.euid, historyTopic, historyAliases, historyStart, historyEnd], reset, { flush: 'sync' });
  onUnmounted(reset);

  async function historyLoad(generate = false) {
    if (historyLoading.value) return;
    if (!store.euid.value.trim() || !historyTopic.value.trim()) {
      historyError.value = '请在顶部选择用户，并输入想了解的话题。';
      return;
    }
    if (historyStart.value && historyEnd.value && historyStart.value > historyEnd.value) {
      historyError.value = '开始日期不能晚于结束日期。';
      return;
    }
    const ticket = ++generation;
    controller = new AbortController();
    historyLoading.value = true;
    historyError.value = '';
    historyStage.value = generate ? '正在翻发言、挑原声，整理这位 JR 的话题小传…' : '正在翻找相关发言与已有小传…';
    try {
      const res = await fetch('/api/history', {
        method: 'POST', signal: controller.signal, headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          euid: store.euid.value.trim(), topic: historyTopic.value.trim(), aliases: historyAliases.value.trim(),
          start: historyStart.value, end: historyEnd.value, generate, refresh: generate && !!historyReport.value,
          api_key: store.userApiKey.value.trim() || undefined,
          provider: store.userApiKey.value.trim() ? store.userApiProvider.value : undefined,
        }),
      });
      const data = await res.json();
      if (!res.ok) throw new Error(data.error || '读取失败，请重试');
      if (ticket !== generation) return;
      historyData.value = data;
      historyEvidence.value = null;
    } catch (e) {
      if (ticket === generation && e.name !== 'AbortError') historyError.value = e.message || '请求失败';
    } finally {
      if (ticket === generation) historyLoading.value = false;
    }
  }
  return {
    historyTopic, historyAliases, historyStart, historyEnd, historyData, historyLoading, historyStage,
    historyError, historyReport, historyClaims, historyClaim, historySource, historyShowClaim,
    historyEvidence, historyEvidenceParts, historyLegacy, historyHighlights, historyLoad,
  };
}
