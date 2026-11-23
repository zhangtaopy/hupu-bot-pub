// Only a quoted PID creates an edge. Nicknames and the order of floors are not identifiers.
export function collectBranch(rootId, children) {
  const rows = [];
  const seen = new Set([rootId]);
  const pending = [{ parent: rootId, depth: 1, index: 0 }];
  while (pending.length) {
    const cursor = pending[pending.length - 1];
    const child = children.get(cursor.parent)?.[cursor.index++];
    if (!child) { pending.pop(); continue; }
    if (seen.has(child.id)) continue;
    seen.add(child.id);
    rows.push({ source: child, parent: cursor.parent, depth: cursor.depth });
    pending.push({ parent: child.id, depth: cursor.depth + 1, index: 0 });
  }
  return rows;
}


export function rankHotComments(hot, children) {
  return hot.map((source, index) => {
    const total = collectBranch(source.id, children).length;
    return { source, index, total, direct: (children.get(source.id) || []).length };
  }).sort((a, b) => b.total - a.total || a.index - b.index);
}

// Only ordinary floors have authoritative page positions; a hot-only supplement can appear last.
export function pageBands(thread, limit = 24) {
  const pages = Math.max(1, thread.pages);
  const count = Math.min(pages, limit);
  const bands = Array.from({ length: count }, (_, i) => ({
    start: Math.floor(i * pages / count) + 1, end: Math.floor((i + 1) * pages / count),
    lights: 0, quotes: 0, floors: 0, top: null,
  }));
  for (const s of thread.sources.slice(1, 1 + thread.floor_count)) {
    if (s.page < 1 || s.page > pages) continue;
    const band = bands[Math.ceil(s.page * count / pages) - 1];
    band.floors++;
    band.lights += s.lights || 0;
    if (s.quotes.length) band.quotes++;
    if (!s.hidden && (!band.top || (s.lights || 0) > (band.top.lights || 0))) band.top = s;
  }
  return bands;
}

// Allocate each subtree an angular wedge by its leaf count. A reply with one child
// stays on the same spoke; siblings never acquire a link just because they agree.
export function radialLayout(root, children) {
  const rows = collectBranch(root.id, children);
  const nodes = new Map([[root.id, { source: root, depth: 0, leaves: 1, children: [] }]]);
  let maxDepth = 0;
  for (const { source, parent, depth } of rows) {
    nodes.set(source.id, { source, depth, leaves: 1, children: [] });
    nodes.get(parent).children.push(source.id);
    maxDepth = Math.max(maxDepth, depth);
  }
  const order = [...nodes.values()];
  for (let i = order.length - 1; i >= 0; i--) {
    const n = order[i];
    if (n.children.length) n.leaves = n.children.reduce((sum, id) => sum + nodes.get(id).leaves, 0);
  }
  const pending = [{ id: root.id, from: -Math.PI / 2, width: 2 * Math.PI }];
  while (pending.length) {
    const { id, from, width } = pending.pop();
    const n = nodes.get(id);
    const angle = from + width / 2;
    const r = maxDepth ? Math.log1p(n.depth) / Math.log1p(maxDepth) : 0;
    n.x = Math.cos(angle) * r;
    n.y = Math.sin(angle) * r;
    let next = from;
    for (const child of n.children) {
      const span = width * nodes.get(child).leaves / n.leaves;
      pending.push({ id: child, from: next, width: span });
      next += span;
    }
  }
  return { nodes: order, edges: rows.map(r => ({ source: r.parent, target: r.source.id })), maxDepth };
}

export const ReplyNetwork = {
  props: ['root', 'childrenIndex', 'positions', 'target', 'filter', 'dark', 'colorFor', 'sources'],
  emits: ['inspect'],
  setup(props, { emit }) {
    const { ref, computed, watch, onMounted, onBeforeUnmount } = Vue;
    const host = ref(null), focusId = ref(''), error = ref('');
    let chart, observer, frame;
    const focus = computed(() => props.sources.get(focusId.value) || props.root);
    const layout = computed(() => radialLayout(focus.value, props.childrenIndex));
    const picked = ref(null);
    function inspect(id) {
      picked.value = id;
      emit('inspect', id);
    }
    function drill() {
      if (props.childrenIndex.get(picked.value)?.length) focusId.value = picked.value;
      picked.value = null;
    }
    function reset() { focusId.value = ''; picked.value = null; draw(true); }
    function draw(resetView = false) {
      if (!chart || !host.value?.clientWidth) return;
      const layoutData = layout.value;
      const colors = getComputedStyle(host.value);
      const color = token => colors.getPropertyValue(token).trim();
      const primary = color('--text-primary'), surface = color('--surface');
      const pendingColor = color('--tm-pending');
      const n = layoutData.nodes.length;
      const byDepth = !Object.keys(props.positions).length;
      const layerColor = depth => depth===1 ? color('--tm-level-one') : depth===2 ? color('--tm-level-two') : color('--tm-level-deep');
      const span = 130;
      const data = layoutData.nodes.map(node => {
        const id = node.source.id, isCenter = id === focus.value.id;
        const stance = props.positions[id]?.[props.target];
        const faded = props.filter && props.filter !== (stance || 'pending') && !isCenter;
        const nodeColor = stance ? props.colorFor(stance) : (byDepth ? layerColor(node.depth) : pendingColor);
        return { id, name: node.source.author || '匿名', x: node.x * span, y: node.y * span,
          symbolSize: isCenter ? 23 : Math.min(21, (n > 150 ? 6.5 : 12) + 2.4 * Math.log1p(node.children.length)),
          itemStyle: { color: isCenter ? color('--tm-ink') : nodeColor,
            opacity: faded ? .17 : 1, borderWidth: isCenter || n < 35 ? 1.8 : .4, borderColor: surface },
          label: { show: isCenter || (n < 13 && node.depth <= 2), formatter: isCenter ? (focus.value.id === props.root.id ? '热评' : '当前回复') : node.source.author?.slice(0,8),
            position:'bottom', color: primary, fontSize:11, backgroundColor:surface, borderRadius:3, padding:[2,4] },
        };
      });
      const links = layoutData.edges.map(edge => ({...edge, lineStyle:{
        opacity: props.filter && (props.positions[edge.target]?.[props.target] || 'pending') !== props.filter ? .09 : .55,
      }}));
      const series = {id:'replies', type:'graph', layout:'none', roam:true, scaleLimit:{min:.35,max:18},
        data, links, animation:false, layoutAnimation:false, draggable:false, labelLayout:{ hideOverlap:true },
        // Fixed coordinate bounds keep small subtrees and straight chains legible.
        left:'10%', right:'10%', top:'12%', bottom:'12%',
        lineStyle:{color:color('--tm-edge'),width:n>250?1.15:1.7,curveness:.08},
        emphasis:{focus:'adjacency',scale:1.3,lineStyle:{width:2.6,opacity:1}},
        tooltip:{formatter:p=>p.dataType==='node' ? p.name+' · '+(props.sources.get(p.data.id)?.text || '无可读文字').slice(0,40) : ''},
      };
      // ECharts rescales graph coordinates to the data bounds. Two invisible anchors
      // give isolated roots and one-sided branches the same centered coordinate system.
      for (const sign of [-1,1]) {
        series.data.push({ id:'bound'+sign, name:'', x:span*sign, y:span*sign, symbolSize:0, silent:true,
          label:{show:false}, tooltip:{show:false}, itemStyle:{opacity:0} });
      }
      if (resetView) { series.zoom=1; series.center=null; }
      chart.setOption({tooltip:{renderMode:'richText',confine:true},series:[series]}, {notMerge:resetView});
    }
    function zoom(factor) {
      if (chart) chart.dispatchAction({type:'graphRoam',seriesId:'replies',zoom:factor,originX:chart.getWidth()/2,originY:chart.getHeight()/2});
    }
    function resize() {
      cancelAnimationFrame(frame);
      frame=requestAnimationFrame(()=>{ chart?.resize(); });
    }
    onMounted(()=>{
      if (!window.echarts) { error.value='关系图组件未加载，可以展开下方回复明细查看引用。'; return; }
      chart=window.echarts.init(host.value,null,{renderer:'canvas'});
      chart.on('click', p=>{ if (p.dataType==='node' && props.sources.has(p.data.id)) inspect(p.data.id); });
      observer=new ResizeObserver(resize);
      observer.observe(host.value);
      draw(true);
    });
    watch(() => props.root.id, () => { focusId.value=''; picked.value=null; });
    watch(layout, () => draw(true), {flush:'post'});
    watch(() => [props.positions, props.target, props.filter, props.dark], () => draw(), {flush:'post'});
    onBeforeUnmount(()=>{ cancelAnimationFrame(frame); observer?.disconnect(); chart?.dispose(); });
    return {host,focus,layout,error,picked,drill,reset,zoom};
  },
  template: `<div class="tm-network">
    <div class="tm-heading"><div><h4>回复关系图</h4><p>{{ layout.nodes.length - 1 }} 条关联回复 · 最深 {{ layout.maxDepth }} 层引用</p></div>
      <div class="tm-map-tools"><button type="button" aria-label="缩小关系图" @click="zoom(.8)">−</button><button type="button" aria-label="放大关系图" @click="zoom(1.25)">＋</button><button type="button" @click="reset">复位</button></div></div>
    <div ref="host" class="tm-map" role="img" :aria-label="'回复关系图，'+(layout.nodes.length-1)+' 条关联回复'"></div>
    <p v-if="error" role="status">{{ error }}</p>
    <div v-if="!Object.keys(positions).length" class="tm-key tm-depth-key"><span><i style="background:var(--tm-level-one)"></i>{{ focus.id===root.id ? '直接回热评' : '直接回复' }}</span><span><i style="background:var(--tm-level-two)"></i>{{ focus.id===root.id ? '第二层' : '下两层' }}</span><span><i style="background:var(--tm-level-deep)"></i>更深的引用</span></div>
    <div class="tm-graph-foot"><span>点节点看原文 · 滚轮缩放 · 连线表示引用</span>
      <button v-if="picked && childrenIndex.get(picked)?.length && picked!==focus.id" class="tm-action" @click="drill">聚焦这条分支</button>
      <button v-else-if="focus.id!==root.id" class="tm-action" @click="reset">回到热评</button></div>
  </div>`,
};
