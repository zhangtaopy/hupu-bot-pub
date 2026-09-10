/**
 * Interaction Graph Tab — 互动图谱（社交图）。
 * 基于 replies 表的引用关系构建"谁引用了谁"的单向关系图：
 *   - 太阳系式布局：本人（紫色）固定在中心，互动对象环绕公转，
 *     互动分数高的星球轨道更靠中心，分数低的更远
 *   - 防重叠（v2）：
 *       1) 星球尺寸随数量全局自适应——星球越多，单个越小（面积预算）
 *       2) 分环按"实际半径 + 同环弦长"排布，环与环之间留出环形间隙
 *       3) 同环刚性公转（整环一起转，环间反向），不再有相对漂移互相压盖
 *       4) 标签智能避让：只画重点且互不压盖，hover 时其余星球淡出聚焦
 *   - 无连线：纯粹的行星轨道视觉
 *   - 圆圈大小 = 互动次数为主 + 点亮为辅
 *   - 节点颜色 = 平均点亮质量热力分档：灰(<1) → 青(1~4) → 蓝(4~10) → 橙(10~25) → 红(≥25)
 *   - 高亮节点带光晕，一眼跳出来
 *   - hover 节点显示最热的几条引用（按点亮数取前 3）
 *   - 点击节点打开右侧详情抽屉，分页查看全部互动回帖
 *
 * 实现要点：
 *   - 抛弃 ECharts 的 graph，改为原生 Canvas 自绘 + requestAnimationFrame，
 *     避免每帧 setOption 带来的巨大开销，确保 60fps 流畅公转。
 *   - 视图支持滚轮缩放、拖拽平移、双击重置。
 *   - 布局算法 computeSolarLayout 是纯函数（不碰 DOM），可脱离浏览器单独验证。
 */

// ═══════════════════════════════════════════════════════════
// 模块级：尺寸 / 分档 / 布局（纯函数，不依赖 DOM，便于单独验证）
// ═══════════════════════════════════════════════════════════

/** 平均点亮质量分档（颜色 + 光晕），从低到高 */
export const LIGHT_BUCKETS = [
  { min: 25, color: '#ff3b30', glow: 'rgba(255,59,48,0.55)', glowBlur: 22 }, // 红：神评制造机
  { min: 10, color: '#ff9f0a', glow: 'rgba(255,159,10,0.5)',  glowBlur: 16 }, // 橙：热评常客
  { min: 4,  color: '#0a84ff', glow: null,                    glowBlur: 0 },  // 蓝：稳定输出
  { min: 1,  color: '#64d2ff', glow: null,                    glowBlur: 0 },  // 青：偶尔一亮
  { min: -Infinity, color: '#98989d', glow: null,             glowBlur: 0 },  // 灰：低亮
];

export function avgLight(node) {
  return node.count > 0 ? node.light_sum / node.count : 0;
}

/** 节点所属的热力分档 */
export function lightBucket(node) {
  return LIGHT_BUCKETS.find(b => avgLight(node) >= b.min) || LIGHT_BUCKETS[LIGHT_BUCKETS.length - 1];
}

/** 圆圈基础直径 = 互动次数为主 + 点亮为辅（高亮也能撑大一点），主节点最大（未做拥挤度缩放） */
export function baseNodeSize(node) {
  const countBoost = Math.sqrt(Math.max(node.count, 0));
  const lightBoost = Math.sqrt(Math.max(node.light_sum, 0));
  if (node.is_main) return Math.min(56 + countBoost * 3.0, 108);
  return Math.min(16 + countBoost * 3.4 + lightBoost * 1.0, 92);
}

/** 互动分数：count 权重高，light_sum 权重低 */
export function interactionScore(node) {
  return node.count * 3 + node.light_sum * 0.5;
}

/** 布局参数（世界坐标 px，围绕 maxR ≈ 290 的圆盘调过） */
export const LAYOUT = {
  gap: 9,          // 同环相邻星球之间的最小空隙
  ringGap: 14,     // 环与环之间的额外空隙
  minScale: 0.34,  // 星球尺寸全局缩放下限
  minSize: 13,     // 星球最小直径
  maxRings: 6,     // 最多环数：环越多越密，宁少勿多
  maxPerRing: 20,  // 单环最多星球数：避免外环挤成一条线
  perRing: 10,     // 每环基准容量（决定环数）
  mainScale: 0.85, // 主节点缩放系数（保持"太阳"的存在感）
  mainMinSize: 48, // 主节点最小直径
};

/**
 * 同一环上等角槽位排布时，保证任意两颗星球不重叠所需的最小半径。
 * 半径 R 的圆上，相隔 d 个槽位（共 m 个）的两个点弦长为 2R·sin(πd/m)，
 * 要求它 ≥ 两球半径之和 + gap（取所有点对的最大需求，即为充分条件）。
 * @param {number[]} rhos 该环每颗星球的半径（按槽位顺序）
 * @param {number} gap 最小空隙
 */
export function ringMinRadius(rhos, gap) {
  const m = rhos.length;
  if (m < 2) return 0;
  let need = 0;
  for (let i = 0; i < m; i++) {
    for (let j = i + 1; j < m; j++) {
      const d = Math.min(j - i, m - (j - i));   // 取短弧方向（d ≥ 1，πd/m ≤ π/2）
      const s = Math.sin((Math.PI * d) / m);
      if (s <= 1e-9) continue;
      need = Math.max(need, (rhos[i] + rhos[j] + gap) / (2 * s));
    }
  }
  return need;
}

/**
 * 太阳系布局：把互动对象按分数从内到外分环，并保证几何上互不重叠。
 *
 * 保证来自两条硬约束：
 *   - 同环：等角槽位 + 弦长校验（ringMinRadius），环内任意两球间距 ≥ 半径和 + gap
 *   - 异环：相邻环半径差 ≥ 两环最大半径 + ringGap，故跨环任意两球也必然分开
 *
 * 星球越多越挤时，不会硬塞，而是按 `scale` 等比缩小所有星球
 * （二分查找最大的可行 scale），实在放不下才从分数最低的尾部裁掉。
 *
 * @param {Array} items 互动对象（需已带 baseSize 与 score）
 * @param {object} opts maxR / mainRadius 以及 LAYOUT 覆盖项
 * @returns {{ rings: Array, scale: number, hidden: Array, mainRadius: number }}
 */
export function computeSolarLayout(items, opts = {}) {
  const cfg = { ...LAYOUT, ...opts };
  const {
    gap, ringGap, minScale, minSize, maxRings, maxPerRing, perRing, maxR,
  } = cfg;
  const mainRadius = cfg.mainRadius || 0;
  const rInner = mainRadius + ringGap;   // 最内环的内边界

  const all = items.slice().sort((a, b) => b.score - a.score);

  // 用给定缩放尝试排布；放不下返回 null
  function attempt(scale, list) {
    const n = list.length;
    if (!n) return { rings: [], scale, outer: rInner };

    const K = Math.max(1, Math.min(maxRings, Math.ceil(n / perRing)));
    // 每环容量：内环少（放最大的星球）、外环多（放小星球），权重 k+2
    const weights = Array.from({ length: K }, (_, k) => k + 2);
    const wsum = weights.reduce((a, b) => a + b, 0);
    const counts = weights.map(w => Math.min(maxPerRing, Math.floor((n * w) / wsum)));
    let assigned = counts.reduce((a, b) => a + b, 0);
    // 余数从外环往里补（内环保持稀疏）
    for (let k = K - 1; k >= 0 && assigned < n; k--) {
      const add = Math.min(n - assigned, maxPerRing - counts[k]);
      counts[k] += add;
      assigned += add;
    }
    if (assigned < n) return null;   // 环容量不足（星球太多）

    const rings = [];
    let idx = 0;
    let prevOuter = rInner;
    for (let k = 0; k < K; k++) {
      const group = list.slice(idx, idx + counts[k]);
      idx += counts[k];
      if (!group.length) continue;
      const rhos = group.map(g => Math.max(minSize / 2, (g.baseSize * scale) / 2));
      const rhoMax = Math.max(...rhos);
      // 贴住上一环外沿 + 满足同环弦长
      const r = Math.max(prevOuter + rhoMax, ringMinRadius(rhos, gap));
      rings.push({ group, rhos, rhoMax, r });
      prevOuter = r + rhoMax + ringGap;
    }

    // 星球少的时候会有大量余量：把余量均摊到各环间距上，让整个星系铺满画布
    // （每环外移量递增，故环间距只增不减，不破坏防重叠约束）
    if (rings.length) {
      const outerRaw = rings[rings.length - 1].r + rings[rings.length - 1].rhoMax;
      const required = outerRaw - rInner;
      const room = Math.min(maxR, rInner + required * 2.2) - outerRaw;   // 最多铺开到 2.2 倍疏密
      if (room > 1) {
        const d = room / (rings.length + 1);
        for (let k = 0; k < rings.length; k++) rings[k].r += d * (k + 1);
      }
    }

    const outer = rings.length ? rings[rings.length - 1].r + rings[rings.length - 1].rhoMax : rInner;
    if (outer > maxR + 1e-6) return null;   // 溢出画布
    return { rings, scale, outer };
  }

  // 空间不够时，从分数最低的尾部裁掉，直到最小缩放下能放下
  let visible = all;
  let hidden = [];
  let base = attempt(minScale, visible);
  while (!base && visible.length > 1) {
    const drop = Math.max(1, Math.ceil(visible.length * 0.05));
    hidden = visible.slice(visible.length - drop).concat(hidden);
    visible = visible.slice(0, visible.length - drop);
    base = attempt(minScale, visible);
  }
  if (!base) return { rings: [], scale: minScale, hidden: all.slice(), mainRadius };

  // 二分查找最大的可行缩放：星球越少 → 单个越大；越多 → 越小，但永不变形重叠
  let lo = minScale;
  let hi = 1;
  let best = base;
  for (let it = 0; it < 12; it++) {
    const mid = (lo + hi) / 2;
    const trial = attempt(mid, visible);
    if (trial) {
      best = trial;
      lo = mid;
    } else {
      hi = mid;
    }
  }

  const K = best.rings.length;
  const rings = best.rings.map((ring, k) => {
    const dir = k % 2 === 0 ? 1 : -1;
    const speed = (0.17 - 0.075 * (k / Math.max(K - 1, 1))) * dir;   // 内环快、外环慢，相邻环反向
    const phase = k * 0.55 + (k % 2 ? 0.26 : 0);                     // 相位错开，避免排成放射线
    const m = ring.group.length;
    return {
      r: ring.r,
      speed,
      nodes: ring.group.map((item, i) => {
        const rho = ring.rhos[i];
        return {
          ...item,
          rho,
          size: rho * 2,
          orbitR: ring.r,
          baseAngle: phase + (i / m) * Math.PI * 2,   // 等角槽位：环内相对位置恒定
          x: 0,
          y: 0,
        };
      }),
    };
  });

  return { rings, scale: best.scale, hidden, mainRadius };
}

// ═══════════════════════════════════════════════════════════
// 组件
// ═══════════════════════════════════════════════════════════

export function setupInteractionGraphTab(store) {
  const CHART_EL_ID = 'interaction-graph-chart';

  // ── DOM 元素 ──
  let containerEl = null;
  let canvas = null;
  let ctx = null;
  let tooltipEl = null;

  // ── 动画 ──
  let rafId = null;
  let clock = 0;          // 累计动画时间（秒），暂停时不增长
  let lastFrameTs = 0;

  // ── 尺寸 ──
  let width = 0;
  let height = 0;
  let dpr = 1;
  let cx = 0;
  let cy = 0;
  let maxR = 0;

  // ── 视图变换（缩放/平移） ──
  let scale = 1;
  let panX = 0;
  let panY = 0;
  let targetScale = 1;
  let targetPanX = 0;
  let targetPanY = 0;

  // ── 交互状态 ──
  let isDragging = false;
  let dragMoved = false;
  let lastMouseX = 0;
  let lastMouseY = 0;
  let hoveredNode = null;

  // ── 图数据 ──
  let mainNode = null;
  let rings = [];       // [{ r, speed, nodes:[{ ..., rho, size, orbitR, baseAngle, x, y }] }]
  let allNodes = [];    // 包含主节点在内的所有渲染节点（每帧按 y 排序）
  let totalTargets = 0;

  function theme() {
    const dark = store.darkMode.value;
    return {
      text: dark ? '#e5e5ea' : '#1c1c1e',
      subText: dark ? '#98989f' : '#6e6e73',
      faint: dark ? '#636366' : '#aeaeb2',
      nodeBorder: dark ? '#1c1c1e' : '#ffffff',
      tooltipBg: dark ? 'rgba(28,28,30,0.96)' : 'rgba(255,255,255,0.97)',
      tooltipText: dark ? '#e5e5ea' : '#1c1c1e',
      tooltipSub: dark ? '#98989f' : '#6e6e73',
      tooltipBorder: dark ? 'rgba(255,255,255,0.12)' : 'rgba(0,0,0,0.08)',
      accent: '#bf5af2',
      orbit: dark ? 'rgba(255,255,255,0.10)' : 'rgba(0,0,0,0.06)',
      bg: dark ? 'rgba(28,28,30,0.35)' : 'rgba(255,255,255,0.45)',
      labelBg: dark ? 'rgba(28,28,30,0.68)' : 'rgba(255,255,255,0.72)',
    };
  }

  function escapeHtml(s) {
    return String(s ?? '')
      .replaceAll('&', '&amp;')
      .replaceAll('<', '&lt;')
      .replaceAll('>', '&gt;')
      .replaceAll('"', '&quot;')
      .replaceAll("'", '&#39;');
  }

  function truncate(s, n) {
    const str = String(s ?? '');
    return str.length > n ? str.slice(0, n) + '…' : str;
  }

  // 节点颜色：平均点亮质量热力分档（本人永远紫色）
  function nodeColor(node) {
    if (node.is_main) return theme().accent;
    return lightBucket(node).color;
  }

  // 高亮节点光晕：红色/橙色档发光，主节点紫色发光
  function nodeGlow(node) {
    if (node.is_main) return { blur: 28, color: 'rgba(191,90,242,0.55)' };
    const bucket = lightBucket(node);
    if (bucket.glow) return { blur: bucket.glowBlur, color: bucket.glow };
    return null;
  }

  // ── DOM 初始化 ──

  function ensureCanvas() {
    const el = document.getElementById(CHART_EL_ID);
    if (!el) return null;

    if (canvas && canvas.parentElement === el) {
      return el;
    }

    // 清理旧内容
    el.innerHTML = '';
    canvas = document.createElement('canvas');
    canvas.style.display = 'block';
    canvas.style.width = '100%';
    canvas.style.height = '100%';
    canvas.style.borderRadius = 'inherit';
    canvas.style.cursor = 'grab';
    el.appendChild(canvas);
    ctx = canvas.getContext('2d', { alpha: true });

    // 绑定事件
    canvas.addEventListener('mousemove', onMouseMove);
    canvas.addEventListener('mouseleave', onMouseLeave);
    canvas.addEventListener('mousedown', onMouseDown);
    canvas.addEventListener('mouseup', onMouseUp);
    canvas.addEventListener('click', onClick);
    canvas.addEventListener('wheel', onWheel, { passive: false });
    canvas.addEventListener('dblclick', onDblClick);

    // tooltip
    if (!tooltipEl) {
      tooltipEl = document.createElement('div');
      tooltipEl.style.position = 'fixed';
      tooltipEl.style.zIndex = '80';
      tooltipEl.style.pointerEvents = 'none';
      tooltipEl.style.opacity = '0';
      tooltipEl.style.transition = 'opacity 0.12s ease';
      document.body.appendChild(tooltipEl);
    }

    return el;
  }

  function sizeCanvas() {
    if (!containerEl || !canvas) return;
    const rect = containerEl.getBoundingClientRect();
    width = rect.width;
    height = rect.height;
    dpr = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = Math.floor(width * dpr);
    canvas.height = Math.floor(height * dpr);
    canvas.style.width = width + 'px';
    canvas.style.height = height + 'px';
    cx = width / 2;
    cy = height / 2;
    // 圆盘半径：高度通常比宽度更紧张，只留出最外圈星球与命中区余量
    maxR = Math.max(120, Math.min(cx, cy) - 54);
  }

  // ── 数据构建 ──

  function buildGraphData(data) {
    totalTargets = data.total_targets || 0;

    // 主节点（太阳）：固定中心，尺寸不随缩放缩太多，保持存在感
    const mainRaw = data.nodes.find(n => n.is_main) || data.nodes[0];
    const mainSize = Math.max(LAYOUT.mainMinSize, baseNodeSize(mainRaw) * LAYOUT.mainScale);
    mainNode = {
      ...mainRaw,
      is_main: true,
      size: mainSize,
      rho: mainSize / 2,
      color: nodeColor(mainRaw),
      glow: nodeGlow(mainRaw),
      orbitR: 0,
      baseAngle: 0,
      x: cx,
      y: cy,
      isTop: true,
      isTop3: true,
    };

    // 非主节点：算好基础尺寸与分数，交给布局算法
    const others = data.nodes
      .filter(n => !n.is_main)
      .map(n => ({
        ...n,
        baseSize: baseNodeSize(n),
        score: interactionScore(n),
        color: nodeColor(n),
        glow: nodeGlow(n),
      }));

    // 标签优先级：互动最多的前 3 / 前 8
    const byCount = others.slice().sort((a, b) => b.count - a.count);
    const top3Names = new Set(byCount.slice(0, 3).map(n => n.name));
    const top8Names = new Set(byCount.slice(0, 8).map(n => n.name));

    const layout = computeSolarLayout(others, {
      maxR,
      mainRadius: mainNode.rho,
      gap: LAYOUT.gap,
      ringGap: LAYOUT.ringGap,
    });

    rings = layout.rings.map(ring => ({
      r: ring.r,
      speed: ring.speed,
      nodes: ring.nodes.map(n => ({
        ...n,
        isTop: top8Names.has(n.name),
        isTop3: top3Names.has(n.name),
        x: cx,
        y: cy,
      })),
    }));

    store.graphHiddenCount.value = layout.hidden.length;
    store.graphNodeScale.value = layout.scale;
    allNodes = [mainNode, ...rings.flatMap(r => r.nodes)];
  }

  // ── 动画循环 ──

  function updateNodePositions(t) {
    // 主节点始终居中
    mainNode.x = cx;
    mainNode.y = cy;

    // 整环刚性公转：环内相对位置恒定 → 同环永远不会相互压盖
    for (const ring of rings) {
      const spin = t * ring.speed;
      for (const n of ring.nodes) {
        const ang = n.baseAngle + spin;
        n.x = cx + n.orbitR * Math.cos(ang);
        n.y = cy + n.orbitR * Math.sin(ang);
        n.labelPos = Math.cos(ang) >= 0 ? 'right' : 'left';
      }
    }

    // 按 y 坐标排序，实现遮挡关系（y 小的在后方）
    allNodes = [mainNode, ...rings.flatMap(r => r.nodes)];
    allNodes.sort((a, b) => a.y - b.y);
  }

  function drawOrbits(t) {
    ctx.save();
    ctx.lineWidth = 1;
    for (let i = 0; i < rings.length; i++) {
      // 内环稍明显，外环更淡，减少"一堆同心圆"的杂乱感
      const alpha = 1 - (i / Math.max(rings.length, 1)) * 0.45;
      ctx.strokeStyle = t.orbit;
      ctx.globalAlpha = alpha;
      ctx.beginPath();
      ctx.arc(cx, cy, rings[i].r, 0, Math.PI * 2);
      ctx.stroke();
    }
    ctx.restore();
  }

  function drawNode(node, t, alpha) {
    const isMain = node.is_main;
    const rho = node.rho;
    const size = isMain ? node.size * (1 + 0.045 * Math.sin(clock * 2.2)) : node.size;

    ctx.save();
    if (alpha < 1) ctx.globalAlpha = alpha;

    // 光晕（随星球尺寸缩放，避免小星球挂着一团大光斑）
    if (node.glow) {
      const k = Math.min(1, Math.max(0.45, size / 42));
      ctx.shadowBlur = isMain
        ? node.glow.blur * (1 + 0.25 * Math.sin(clock * 2.2))
        : node.glow.blur * k;
      ctx.shadowColor = node.glow.color;
    }

    // 主体圆
    ctx.beginPath();
    ctx.arc(node.x, node.y, size / 2, 0, Math.PI * 2);
    if (isMain) {
      const grad = ctx.createRadialGradient(
        node.x - size * 0.15,
        node.y - size * 0.15,
        size * 0.1,
        node.x,
        node.y,
        size / 2,
      );
      grad.addColorStop(0, '#d5aaff');
      grad.addColorStop(0.5, '#bf5af2');
      grad.addColorStop(1, '#7c3aed');
      ctx.fillStyle = grad;
    } else {
      ctx.fillStyle = node.color;
    }
    ctx.fill();

    // 边框
    ctx.shadowBlur = 0;
    ctx.strokeStyle = t.nodeBorder;
    ctx.lineWidth = isMain ? 2.5 : Math.min(1.5, Math.max(0.8, rho * 0.12));
    ctx.stroke();

    // 主节点内的小高光
    if (isMain) {
      ctx.beginPath();
      ctx.arc(node.x - size * 0.18, node.y - size * 0.18, size * 0.12, 0, Math.PI * 2);
      ctx.fillStyle = 'rgba(255,255,255,0.22)';
      ctx.fill();
    }

    // 右上角高亮提示点：红色档（神评制造机）加一个小亮点
    if (!isMain && node.glow && rho > 7) {
      ctx.beginPath();
      ctx.arc(node.x + rho * 0.45, node.y - rho * 0.55, Math.max(1.4, rho * 0.16), 0, Math.PI * 2);
      ctx.fillStyle = 'rgba(255,255,255,0.9)';
      ctx.fill();
    }

    ctx.restore();
  }

  // ── 标签排布（避让） ──

  function rectCircleHit(rect, cxx, cyy, r, pad) {
    const nx = Math.max(rect.x, Math.min(cxx, rect.x + rect.w));
    const ny = Math.max(rect.y, Math.min(cyy, rect.y + rect.h));
    const dx = cxx - nx;
    const dy = cyy - ny;
    const rr = r + pad;
    return dx * dx + dy * dy < rr * rr;
  }

  function rectsOverlap(a, b, pad) {
    return !(a.x + a.w + pad < b.x || b.x + b.w + pad < a.x
      || a.y + a.h + pad < b.y || b.y + b.h + pad < a.y);
  }

  /** 候选标签按优先级排序：主节点 > hover > 前3 > 前8 > 大头星球 */
  function labelCandidates(mode) {
    const list = [];
    for (const node of allNodes) {
      const isHovered = hoveredNode && hoveredNode.name === node.name;
      const isMain = !!node.is_main;
      const big = node.rho >= 15;   // 直径 ≥30px 的星球

      let pri;
      if (isMain) pri = 1000;
      else if (isHovered) pri = 900;
      else if (node.isTop3) pri = 600 + node.count;
      else if (node.isTop) pri = 400 + node.count;
      else if (big) pri = 200 + node.count;
      else pri = node.count;

      // 自动模式只画"值得看"的：主节点 / hover / 互动前列 / 大星球 / 放大后的小星球
      const autoShow = isMain || isHovered || node.isTop3 || node.isTop || big
        || (scale > 2.0 && node.rho >= 9)
        || scale > 3.0;
      if (mode === 'auto' && !autoShow) continue;

      list.push({ node, pri, isHovered, isMain });
    }
    return list.sort((a, b) => b.pri - a.pri);
  }

  function drawLabels(t) {
    const mode = store.graphLabelMode.value;   // auto | all | off
    if (mode === 'off') return;

    const candidates = labelCandidates(mode);
    const maxLabels = mode === 'all' ? 40 : 14;   // 硬上限：宁可少画，不要糊成一片
    const placedRects = [];
    ctx.save();
    ctx.textBaseline = 'middle';

    for (const { node, isHovered, isMain } of candidates) {
      if (placedRects.length >= maxLabels) break;

      const fontSize = isMain ? 13 : (node.isTop3 || isHovered ? 11.5 : 10.5);
      const fontWeight = isMain || node.isTop3 || isHovered ? 'bold' : '500';
      ctx.font = `${fontWeight} ${fontSize}px "Segoe UI", "Helvetica Neue", "PingFang SC", "Microsoft YaHei", sans-serif`;

      const label = node.name;
      const metrics = ctx.measureText(label);
      const textH = fontSize + 4;
      const padX = 5;
      const padY = 2;
      const bgW = metrics.width + padX * 2;
      const bgH = textH + padY * 2;
      const off = node.rho + 6;   // 与圆圈的距离

      // 依次尝试的摆放方向：跟随角度的左右 → 另一侧 → 上下
      const sides = isMain
        ? ['bottom', 'top', 'right', 'left']
        : (node.labelPos === 'right'
          ? ['right', 'left', 'bottom', 'top']
          : ['left', 'right', 'bottom', 'top']);

      let geo = null;
      for (const side of sides) {
        let tx, ty, align, rect;
        if (side === 'right' || side === 'left') {
          align = side === 'right' ? 'left' : 'right';
          tx = side === 'right' ? node.x + off : node.x - off;
          ty = node.y;
          rect = { x: side === 'right' ? tx : tx - bgW, y: ty - bgH / 2, w: bgW, h: bgH };
        } else {
          align = 'center';
          ty = side === 'bottom' ? node.y + off + bgH / 2 : node.y - off - bgH / 2;
          tx = node.x;
          rect = { x: tx - bgW / 2, y: ty - bgH / 2, w: bgW, h: bgH };
        }

        // 压到已放置的标签？
        let bad = placedRects.some(r => rectsOverlap(rect, r, 3));
        // 压到别的星球？
        if (!bad) {
          for (const other of allNodes) {
            if (other === node) continue;
            if (rectCircleHit(rect, other.x, other.y, other.rho, 4)) { bad = true; break; }
          }
        }
        if (!bad) { geo = { side, tx, ty, align, rect }; break; }
      }

      if (!geo) continue;   // 四个方向都放不下 → 这条标签不画，保持画面干净
      placedRects.push(geo.rect);

      // hover 聚焦时，非当前星球的标签一起淡出，避免抢视线
      ctx.globalAlpha = (!hoveredNode || isHovered || isMain) ? 1 : 0.32;

      // 文字背景块
      ctx.fillStyle = isHovered ? t.labelBg : t.bg;
      if (ctx.roundRect) {
        ctx.beginPath();
        ctx.roundRect(geo.rect.x, geo.rect.y, bgW, bgH, 5);
        ctx.fill();
      } else {
        ctx.fillRect(geo.rect.x, geo.rect.y, bgW, bgH);
      }

      // 文字描边 + 填充
      ctx.strokeStyle = t.nodeBorder;
      ctx.lineWidth = 2;
      ctx.textAlign = geo.align;
      ctx.strokeText(label, geo.tx, geo.ty);
      ctx.fillStyle = isMain ? t.accent : (isHovered ? t.text : t.subText);
      ctx.fillText(label, geo.tx, geo.ty);
    }

    ctx.restore();
  }

  function drawHover() {
    if (!hoveredNode) return;
    const t = theme();
    ctx.save();
    ctx.beginPath();
    ctx.arc(hoveredNode.x, hoveredNode.y, hoveredNode.rho + 8, 0, Math.PI * 2);
    ctx.strokeStyle = t.accent;
    ctx.lineWidth = 2;
    ctx.setLineDash([4, 4]);
    ctx.stroke();
    ctx.restore();
  }

  function draw() {
    if (!ctx || !canvas) return;

    const now = performance.now();
    const dt = lastFrameTs ? Math.min((now - lastFrameTs) / 1000, 0.06) : 0;
    lastFrameTs = now;
    if (!store.graphPaused.value) clock += dt;

    const t = theme();

    // 平滑视图变换
    scale += (targetScale - scale) * 0.12;
    panX += (targetPanX - panX) * 0.12;
    panY += (targetPanY - panY) * 0.12;

    updateNodePositions(clock);

    // 清空画布
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);

    // 应用视图变换
    ctx.setTransform(scale * dpr, 0, 0, scale * dpr, panX * dpr, panY * dpr);

    // 绘制轨道
    drawOrbits(t);

    // 绘制节点（已按 y 排序）；hover 时其余星球淡出，聚焦当前星球
    for (const node of allNodes) {
      const alpha = !hoveredNode || node === hoveredNode
        ? 1
        : (node.is_main ? 0.5 : 0.26);
      drawNode(node, t, alpha);
    }

    // 绘制标签（最上层）
    drawLabels(t);

    // hover 高亮环
    drawHover();

    rafId = requestAnimationFrame(draw);
  }

  // ── 交互 ──

  function worldFromMouse(mx, my) {
    return {
      x: (mx - panX) / scale,
      y: (my - panY) / scale,
    };
  }

  function hitTest(mx, my) {
    const p = worldFromMouse(mx, my);
    let best = null;
    let bestDist = Infinity;
    for (const node of allNodes) {
      const r = node.rho + 4;   // 稍微扩大命中区域
      const dx = p.x - node.x;
      const dy = p.y - node.y;
      const dist2 = dx * dx + dy * dy;
      if (dist2 > r * r) continue;
      // 取圆心最近的；完全重合时以 y 更大（更靠前）的为准
      if (dist2 < bestDist - 0.5 || (Math.abs(dist2 - bestDist) <= 0.5 && best && node.y > best.y)) {
        best = node;
        bestDist = dist2;
      }
    }
    return best;
  }

  function onMouseMove(e) {
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;

    if (isDragging) {
      const dx = e.clientX - lastMouseX;
      const dy = e.clientY - lastMouseY;
      if (Math.abs(dx) > 2 || Math.abs(dy) > 2) dragMoved = true;
      targetPanX += dx;
      targetPanY += dy;
      lastMouseX = e.clientX;
      lastMouseY = e.clientY;
      hideTooltip();
      return;
    }

    hoveredNode = hitTest(mx, my);
    canvas.style.cursor = hoveredNode ? 'pointer' : 'grab';

    if (hoveredNode) {
      showTooltip(hoveredNode, e.clientX, e.clientY);
    } else {
      hideTooltip();
    }
  }

  function onMouseLeave() {
    isDragging = false;
    hoveredNode = null;
    hideTooltip();
    if (canvas) canvas.style.cursor = 'grab';
  }

  function onMouseDown(e) {
    if (!canvas) return;
    isDragging = true;
    dragMoved = false;
    lastMouseX = e.clientX;
    lastMouseY = e.clientY;
    canvas.style.cursor = 'grabbing';
  }

  function onMouseUp() {
    if (!canvas) return;
    isDragging = false;
    canvas.style.cursor = hoveredNode ? 'pointer' : 'grab';
  }

  function onClick(e) {
    if (!canvas) return;
    if (dragMoved) return; // 拖拽时不触发点击
    const rect = canvas.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    const node = hitTest(mx, my);
    if (node) {
      openDetail(node.name, !!node.is_main, node);
    }
  }

  function onWheel(e) {
    if (!canvas) return;
    e.preventDefault();
    const rect = canvas.getBoundingClientRect();
    const mx = e.clientX - rect.left;
    const my = e.clientY - rect.top;
    const zoomFactor = e.deltaY < 0 ? 1.12 : 0.89;
    const newScale = Math.max(0.25, Math.min(6, targetScale * zoomFactor));

    // 以鼠标位置为缩放中心
    const p = worldFromMouse(mx, my);
    targetPanX = mx - p.x * newScale;
    targetPanY = my - p.y * newScale;
    targetScale = newScale;
  }

  function onDblClick() {
    resetView();
  }

  function resetView() {
    targetScale = 1;
    targetPanX = 0;
    targetPanY = 0;
  }

  // ── Tooltip ──

  function quotesHtml(quotes) {
    if (!quotes || !quotes.length) return '';
    const t = theme();
    const items = quotes.map(q => {
      const meta = [];
      if (q.light_count > 0) meta.push(`亮 ${q.light_count}`);
      if (q.format_time) meta.push(q.format_time);
      const quoteLine = q.quote_content
        ? `<div style="font-size:11px;color:${t.faint};margin-top:3px">↩ ${escapeHtml(truncate(q.quote_content, 60))}</div>`
        : '';
      return `<div style="padding:5px 0;border-top:1px solid ${t.tooltipBorder}">
        <div style="font-size:12px;line-height:1.45">${escapeHtml(truncate(q.content, 90))}</div>
        ${quoteLine}
        ${meta.length ? `<div style="font-size:10.5px;color:${t.tooltipSub};margin-top:2px">${meta.join(' · ')}</div>` : ''}
      </div>`;
    }).join('');
    return `<div style="margin-top:6px">${items}</div>`;
  }

  function tooltipHtml(node) {
    const t = theme();
    const base = `style="background:${t.tooltipBg};color:${t.tooltipText};border:1px solid ${t.tooltipBorder};border-radius:12px;padding:10px 12px;max-width:340px;white-space:normal;box-shadow:0 8px 24px rgba(0,0,0,0.18);font-size:13px;line-height:1.5"`;

    if (node.is_main) {
      return `<div ${base}>
        <div style="font-weight:600;color:${t.accent}">${escapeHtml(node.name)}（本人）</div>
        <div style="font-size:11.5px;color:${t.tooltipSub};margin-top:3px">共 ${node.count} 条回帖 · 获亮 ${node.light_sum} · 与 ${totalTargets} 人互动</div>
        ${quotesHtml(node.top_quotes)}
        <div style="font-size:10.5px;color:${t.faint};margin-top:6px">历史最热回帖 · 点击查看</div>
      </div>`;
    }

    const avg = node.count > 0 ? (node.light_sum / node.count).toFixed(1) : '0';
    return `<div ${base}>
      <div style="font-weight:600;display:flex;align-items:center"><span style="display:inline-block;width:9px;height:9px;border-radius:50%;background:${node.color};margin-right:7px;flex-shrink:0"></span>${escapeHtml(node.name)}</div>
      <div style="font-size:11.5px;color:${t.tooltipSub};margin-top:3px">被引用 ${node.count} 次 · 共 ${node.light_sum} 亮 · 平均 ${avg} 亮/次 · ${escapeHtml(node.first_time)} ~ ${escapeHtml(node.last_time)}</div>
      ${quotesHtml(node.top_quotes)}
      <div style="font-size:10.5px;color:${t.faint};margin-top:6px">点击查看全部互动</div>
    </div>`;
  }

  function showTooltip(node, clientX, clientY) {
    if (!tooltipEl) return;
    tooltipEl.innerHTML = tooltipHtml(node);
    tooltipEl.style.opacity = '1';

    // 避免 tooltip 超出视口
    const rect = tooltipEl.getBoundingClientRect();
    const pad = 12;
    let left = clientX + pad;
    let top = clientY + pad;
    if (left + rect.width > window.innerWidth - pad) {
      left = clientX - rect.width - pad;
    }
    if (top + rect.height > window.innerHeight - pad) {
      top = clientY - rect.height - pad;
    }
    tooltipEl.style.left = `${left}px`;
    tooltipEl.style.top = `${top}px`;
  }

  function hideTooltip() {
    if (!tooltipEl) return;
    tooltipEl.style.opacity = '0';
  }

  // ── 生命周期 ──

  function startAnimation() {
    if (rafId) cancelAnimationFrame(rafId);
    lastFrameTs = 0;
    rafId = requestAnimationFrame(draw);
  }

  function stopAnimation() {
    if (rafId) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
  }

  function clearGraph() {
    stopAnimation();
    if (canvas) {
      canvas.removeEventListener('mousemove', onMouseMove);
      canvas.removeEventListener('mouseleave', onMouseLeave);
      canvas.removeEventListener('mousedown', onMouseDown);
      canvas.removeEventListener('mouseup', onMouseUp);
      canvas.removeEventListener('click', onClick);
      canvas.removeEventListener('wheel', onWheel);
      canvas.removeEventListener('dblclick', onDblClick);
    }
    if (containerEl) {
      containerEl.innerHTML = '';
    }
    canvas = null;
    ctx = null;
    containerEl = null;
    rings = [];
    allNodes = [];
    mainNode = null;
    hoveredNode = null;
  }

  function renderGraph() {
    if (!store.graphData.value) return;

    // 保存旧视图与公转相位，resize / 切 Tab 时不跳变
    const prevView = { scale, panX, panY, targetScale, targetPanX, targetPanY };
    const firstRender = !rings.length;
    const viewMoved = prevView.scale !== 1 || prevView.panX !== 0 || prevView.panY !== 0;

    clearGraph();
    containerEl = ensureCanvas();
    if (!containerEl) return;

    sizeCanvas();
    buildGraphData(store.graphData.value);

    if (firstRender) clock = 0;

    if (viewMoved) {
      scale = prevView.scale;
      panX = prevView.panX;
      panY = prevView.panY;
      targetScale = prevView.targetScale;
      targetPanX = prevView.targetPanX;
      targetPanY = prevView.targetPanY;
    } else {
      resetView();
    }

    startAnimation();
  }

  let resizeTimer = null;
  function onWindowResize() {
    if (!canvas || !containerEl) return;
    if (resizeTimer) clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      if (store.graphData.value && document.getElementById(CHART_EL_ID)) {
        renderGraph();
      }
    }, 200);
  }
  window.addEventListener('resize', onWindowResize);

  // ── 工具栏动作 ──

  function setGraphLabelMode(mode) {
    store.graphLabelMode.value = mode;
  }

  function toggleGraphPause() {
    store.graphPaused.value = !store.graphPaused.value;
  }

  function resetGraphView() {
    resetView();
  }

  async function onGraphNodeLimitChange() {
    if (!store.graphData.value) return;
    await loadGraph();
  }

  // ── 数据加载与详情抽屉 ──

  async function loadGraph() {
    const euid = store.euid.value.trim();
    if (!euid || store.graphLoading.value) return;
    store.graphLoading.value = true;
    store.graphError.value = '';
    store.graphDetail.value = null;
    const limit = store.graphNodeLimit.value || 60;
    try {
      const res = await fetch(`/api/interactions/graph?euid=${encodeURIComponent(euid)}&max_nodes=${limit}`);
      if (!res.ok) {
        const text = await res.text();
        throw new Error(text || `请求失败 (HTTP ${res.status})`);
      }
      const data = await res.json();
      if (!data.nodes || !data.nodes.length) {
        throw new Error('该用户没有引用互动数据，请先抓取回帖');
      }
      store.graphData.value = data;
    } catch (e) {
      store.graphData.value = null;
      store.graphError.value = e.message || '生成图谱失败';
    } finally {
      store.graphLoading.value = false;
    }
    if (store.graphData.value) {
      rings = [];   // 重新生成 → 公转相位重置
      await Vue.nextTick();
      renderGraph();
    }
  }

  async function openDetail(name, isMain, nodeInfo) {
    const euid = store.euid.value.trim();
    if (!euid || !name) return;

    if (isMain) {
      store.graphDetail.value = {
        name,
        isMain: true,
        total: 0,
        replies: [],
        offset: 0,
        count: nodeInfo?.count ?? 0,
        light_sum: nodeInfo?.light_sum ?? 0,
        first_time: nodeInfo?.first_time ?? '',
        last_time: nodeInfo?.last_time ?? '',
        top_quotes: nodeInfo?.top_quotes ?? [],
      };
      return;
    }

    store.graphDetail.value = {
      name,
      isMain: false,
      total: null,
      replies: [],
      offset: 0,
      count: nodeInfo?.count ?? 0,
      light_sum: nodeInfo?.light_sum ?? 0,
      first_time: nodeInfo?.first_time ?? '',
      last_time: nodeInfo?.last_time ?? '',
      top_quotes: [],
    };
    store.graphDetailLoading.value = true;
    try {
      const res = await fetch(`/api/interactions/detail?euid=${encodeURIComponent(euid)}&target=${encodeURIComponent(name)}&limit=20&offset=0`);
      if (!res.ok) throw new Error(`请求失败 (HTTP ${res.status})`);
      const data = await res.json();
      store.graphDetail.value.total = data.total;
      store.graphDetail.value.replies = data.replies || [];
      store.graphDetail.value.offset = (data.replies || []).length;
    } catch (e) {
      store.graphDetail.value.replies = [];
      store.graphDetail.value.total = 0;
    } finally {
      store.graphDetailLoading.value = false;
    }
  }

  async function loadMoreDetail() {
    const d = store.graphDetail.value;
    const euid = store.euid.value.trim();
    if (!d || d.isMain || store.graphDetailMoreLoading.value) return;
    if (d.offset >= d.total) return;

    store.graphDetailMoreLoading.value = true;
    try {
      const res = await fetch(`/api/interactions/detail?euid=${encodeURIComponent(euid)}&target=${encodeURIComponent(d.name)}&limit=20&offset=${d.offset}`);
      if (!res.ok) throw new Error(`请求失败 (HTTP ${res.status})`);
      const data = await res.json();
      d.replies = d.replies.concat(data.replies || []);
      d.offset = d.replies.length;
    } catch (e) {
      /* 忽略加载更多失败 */
    } finally {
      store.graphDetailMoreLoading.value = false;
    }
  }

  function closeDetail() {
    store.graphDetail.value = null;
  }

  function reset() {
    clearGraph();
    closeDetail();
    clock = 0;
    store.graphData.value = null;
    store.graphError.value = '';
    store.graphHiddenCount.value = 0;
  }

  return {
    loadGraph,
    renderGraph,
    openDetail,
    closeDetail,
    loadMoreDetail,
    reset,
    nodeColor,
    // 工具栏
    setGraphLabelMode,
    toggleGraphPause,
    resetGraphView,
    onGraphNodeLimitChange,
  };
}
