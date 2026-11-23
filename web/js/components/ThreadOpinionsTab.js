import { collectBranch, pageBands, rankHotComments } from './ThreadCharts.js';

const categories = [
  { id: 'support', label: '支持', color: '#188871' },
  { id: 'oppose', label: '反对', color: '#d55c51' },
  { id: 'mixed', label: '部分认同', color: '#d19a37' },
  { id: 'other', label: '补充 / 接梗', color: '#6186c4' },
  { id: 'unclear', label: '看不出来', color: '#838897' },
];

export const ThreadSource = {
  props: ['source'],
  template: `<div class="text-sm space-y-2 break-words">
    <a class="underline" :href="source.url" target="_blank" rel="noopener noreferrer">{{ source.author || '匿名' }}{{ source.by_op ? ' · 楼主' : '' }} · 原文 ↗</a>
    <p class="whitespace-pre-wrap leading-relaxed">{{ source.text || (source.hidden ? '这条回复已不可见。' : '没有文字说明。') }}</p>
    <a v-for="(m,i) in source.media" :key="i" :href="m.url" target="_blank" rel="noopener noreferrer" class="apple-btn apple-btn-sm apple-btn-gray">{{ m.caption || '查看' + m.kind }}</a>
    <blockquote v-for="(q,i) in source.quotes" :key="i" class="thread-quote">
      <b>引用 {{ q.author || '匿名' }}</b><p class="whitespace-pre-wrap">{{ q.text || '没有文字说明。' }}</p>
      <a v-for="(m,k) in q.media" :key="k" :href="m.url" target="_blank" rel="noopener noreferrer" class="underline mr-2">{{ m.caption || '查看' + m.kind }}</a>
    </blockquote>
  </div>`,
};

function material(source) {
  return [source.text, ...source.quotes.map(q => '引用上文（' + q.author + '）：' + q.text)].join('\n');
}

export function setupThreadOpinionsTab(store) {
  const { ref, computed, watch } = Vue;
  const threadInput = ref(''), threadData = ref(null), threadBusy = ref(''), threadError = ref('');
  const selectedHot = ref(''), threadPage = ref(1), branchPage = ref(1), stanceFilter = ref('');
  const branchBusy = ref(false), branchError = ref('');
  const stanceTarget = ref('root'), inspected = ref(''), showRows = ref(false);
  const bandMetric = ref('lights'), activeBand = ref(null);
  const results = ref({});
  let current, branchRequest;

  const threadSources = computed(() => new Map((threadData.value?.sources || []).map(s => [s.id, s])));
  const hotComments = computed(() => (threadData.value?.hot_ids || []).map(id => threadSources.value.get(id)).filter(Boolean));
  const hotRoot = computed(() => threadSources.value.get(selectedHot.value));
  const children = computed(() => {
    const index = new Map();
    for (const source of threadSources.value.values()) {
      const parent = source.quotes[0]?.id;
      if (!parent) continue;
      if (!index.has(parent)) index.set(parent, []);
      index.get(parent).push(source);
    }
    return index;
  });
  const branchRows = computed(() => collectBranch(selectedHot.value, children.value));
  const positions = computed(() => results.value[selectedHot.value] || {});
  const classifiedCount = computed(() => Object.keys(positions.value).length);
  const rank = computed(() => rankHotComments(hotComments.value, children.value).slice(0, 6));
  const bands = computed(() => threadData.value ? pageBands(threadData.value) : []);
  const maxBand = computed(() => Math.max(0,...bands.value.map(b => b[bandMetric.value])));
  const bandIndex = computed(() => activeBand.value ?? bands.value.findIndex(b=>b[bandMetric.value]===maxBand.value));
  const chosenBand = computed(() => bands.value[bandIndex.value]);
  const stanceCounts = computed(() => {
    const values = Object.values(positions.value);
    return [...categories.map(c => ({ ...c, count: values.filter(p => p[stanceTarget.value] === c.id).length })),
      {id:'pending', label:'待解读', color:'var(--tm-pending)', count:branchRows.value.length-values.length}];
  });
  const donut = computed(() => {
    const total = branchRows.value.length;
    if (!total) return 'var(--fill-secondary)';
    let sum=0;
    const slices = stanceCounts.value.filter(c=>c.count>0).map(c=>{
      const start=sum;
      sum+=c.count/total*100;
      return c.color+' '+start+'% '+sum+'%';
    });
    return 'conic-gradient('+slices.join(',')+')';
  });
  const inspectedSource = computed(() => threadSources.value.get(inspected.value));
  const filtered = computed(() => branchRows.value.filter(r => !stanceFilter.value || (positions.value[r.source.id]?.[stanceTarget.value] || 'pending') === stanceFilter.value));
  function selectStance(id) {
    stanceFilter.value = stanceFilter.value === id ? '' : id;
    showRows.value = !!stanceFilter.value;
  }
  function clickDonut(event) {
    const box=event.currentTarget.getBoundingClientRect();
    const x=event.clientX-box.left-box.width/2, y=event.clientY-box.top-box.height/2;
    if (Math.hypot(x,y) < box.width*.33) { selectStance(''); return; }
    const fraction=(Math.atan2(x,-y)/(2*Math.PI)+1)%1;
    let sum=0;
    for (const c of stanceCounts.value) {
      sum+=c.count;
      if (fraction*branchRows.value.length<sum) {selectStance(c.id);break;}
    }
  }
  function readExample() { threadInput.value='638890883'; return loadThread(false); }
  const branchPages = computed(() => Math.ceil(filtered.value.length / 20));
  const visibleBranch = computed(() => filtered.value.slice((branchPage.value - 1) * 20, branchPage.value * 20));
  const floorPages = computed(() => Math.ceil(((threadData.value?.sources.length || 1) - 1) / 25));
  const visibleFloors = computed(() => threadData.value?.sources.slice(1 + (threadPage.value - 1) * 25, 1 + threadPage.value * 25));
  const stanceName = id => categories.find(c => c.id === id)?.label || '待解读';
  const stanceColor = id => categories.find(c => c.id === id)?.color || 'var(--separator)';

  function stopBranch() {
    branchRequest?.abort();
    branchRequest = null;
    branchBusy.value = false;
    branchError.value = '';
    branchPage.value = 1;
    stanceFilter.value = ''; inspected.value = ''; showRows.value = false;
  }
  watch(selectedHot, stopBranch, { flush: 'sync' });
  watch(stanceTarget, () => { stanceFilter.value = ''; });
  watch(bandMetric, () => { activeBand.value=null; });
  watch(stanceFilter, () => { branchPage.value = 1; });
  function reset() {
    current?.abort();
    current = null;
    stopBranch();
    selectedHot.value = '';
    results.value = {};
    threadPage.value = 1; activeBand.value = null;
    threadData.value = null;
    threadBusy.value = '';
    threadError.value = '';
  }
  watch(threadInput, reset, { flush: 'sync' });
  const credentials = () => ({ api_key:store.userApiKey.value.trim() || undefined, provider:store.userApiProvider.value });

  async function readBranch() {
    if (!hotRoot.value || branchBusy.value) return;
    const root = hotRoot.value;
    const missing = branchRows.value.filter(row => !positions.value[row.source.id]);
    if (!missing.length) return;
    const request = new AbortController();
    branchRequest = request;
    branchBusy.value = true;
    branchError.value = '';
    let cursor = 0;
    async function worker() {
      while (cursor < missing.length && !request.signal.aborted) {
        const rows = missing.slice(cursor, cursor + 20);
        cursor += rows.length;
        const items = rows.map(({source, parent}) => ({
          id: source.id, parent_id: parent, text: source.text,
          parent_text: parent === root.id ? '' : material(threadSources.value.get(parent)),
        }));
        const res = await fetch('/api/thread/branch', {
          method:'POST', signal:request.signal, headers:{'Content-Type':'application/json'},
          body:JSON.stringify({title:threadData.value.title,root_id:root.id,root_text:material(root),items,...credentials()}),
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || '分支解读失败');
        if (request !== branchRequest || request.signal.aborted) return;
        results.value[root.id] = { ...results.value[root.id], ...data.stances };
      }
    }
    try {
      await Promise.all([worker(),worker()]);
    } catch (error) {
      if (request === branchRequest) {
        branchError.value = error.message || '读取失败';
        request.abort();
      }
    } finally {
      if (request === branchRequest) branchBusy.value = false;
    }
  }

  async function loadThread(generate) {
    if (threadBusy.value || !threadInput.value.trim()) return;
    reset();
    const request = new AbortController();
    current = request;
    threadBusy.value = generate ? '正在翻完整栋楼、读主帖和热评。长帖会分批阅读…' : '正在逐页收集楼层和引用…';
    try {
      const res = await fetch('/api/thread/opinions', {
        method:'POST', signal:request.signal, headers:{'Content-Type':'application/json'},
        body:JSON.stringify({post:threadInput.value.trim(), generate, cookie:store.userCookie.value.trim() || undefined,
          ...(generate ? credentials() : {})}),
      });
      const data = await res.json();
      if (!res.ok) throw new Error(data.error || '读取帖子失败');
      if (request !== current) return;
      threadData.value = data;
      threadError.value = [data.analysis_error,data.hot_error].filter(Boolean).join(' · ');
      selectedHot.value = data.hot_ids[0] || '';
      if (generate && data.review && selectedHot.value) void readBranch();
    } catch (error) {
      if (request === current) threadError.value = error.message || '读取失败，请重试';
    } finally {
      if (request === current) threadBusy.value = '';
    }
  }

  return { threadInput, threadData, threadBusy, threadError, threadSources, loadThread,
    selectedHot, hotComments, hotRoot, branchRows, visibleBranch, branchPages, branchPage,
    stanceCounts, stanceFilter, stanceName, stanceColor, positions, classifiedCount, branchBusy, branchError, readBranch,
    children, visibleFloors, threadPage, floorPages,
    rank, bands, maxBand, bandIndex, chosenBand, bandMetric, activeBand,
    stanceTarget, donut, inspected, inspectedSource, showRows, selectStance, clickDonut, readExample };
}
