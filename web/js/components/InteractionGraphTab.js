/**
 * Interaction Graph Tab — 互动图谱（社交图）。
 * 基于 replies 表的引用关系构建"谁引用了谁"的单向关系图：
 *   - 太阳系式布局：本人（紫色）固定在中心，互动对象环绕公转，
 *     互动分数高的星球轨道更靠中心，分数低的更远
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
 */
export function setupInteractionGraphTab(store) {
  const CHART_EL_ID = 'interaction-graph-chart';

  // ── DOM 元素 ──
  let containerEl = null;
  let canvas = null;
  let ctx = null;
  let tooltipEl = null;

  // ── 动画 ──
  let rafId = null;
  let startTime = 0;

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
  let rings = [];       // [{ r, speed, nodes:[{ ... baseAngle, speed, x, y }] }]
  let allNodes = [];    // 包含主节点在内的所有渲染节点（每帧按 y 排序）
  let totalTargets = 0;

  // 平均点亮质量分档（颜色 + 光晕），从低到高
  const LIGHT_BUCKETS = [
    { min: 25, color: '#ff3b30', glow: 'rgba(255,59,48,0.55)', glowBlur: 22 }, // 红：神评制造机
    { min: 10, color: '#ff9f0a', glow: 'rgba(255,159,10,0.5)',  glowBlur: 16 }, // 橙：热评常客
    { min: 4,  color: '#0a84ff', glow: null,                    glowBlur: 0 },  // 蓝：稳定输出
    { min: 1,  color: '#64d2ff', glow: null,                    glowBlur: 0 },  // 青：偶尔一亮
    { min: -Infinity, color: '#98989d', glow: null,             glowBlur: 0 },  // 灰：低亮
  ];

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
      orbit: dark ? 'rgba(255,255,255,0.12)' : 'rgba(0,0,0,0.07)',
      bg: dark ? 'rgba(28,28,30,0.35)' : 'rgba(255,255,255,0.45)',
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

  function avgLight(node) {
    return node.count > 0 ? node.light_sum / node.count : 0;
  }

  // 节点颜色：平均点亮质量热力分档（本人永远紫色）
  function nodeColor(node) {
    if (node.is_main) return theme().accent;
    const bucket = LIGHT_BUCKETS.find(b => avgLight(node) >= b.min);
    return bucket ? bucket.color : '#98989d';
  }

  // 高亮节点光晕：红色/橙色档发光，主节点紫色发光
  function nodeGlow(node) {
    if (node.is_main) return { blur: 28, color: 'rgba(191,90,242,0.55)' };
    const bucket = LIGHT_BUCKETS.find(b => avgLight(node) >= b.min);
    if (bucket && bucket.glow) return { blur: bucket.glowBlur, color: bucket.glow };
    return null;
  }

  // 圆圈大小 = 互动次数为主 + 点亮为辅（高亮也能撑大一点），主节点最大
  function nodeSize(node) {
    const countBoost = Math.sqrt(Math.max(node.count, 0));
    const lightBoost = Math.sqrt(Math.max(node.light_sum, 0));
    if (node.is_main) return Math.min(56 + countBoost * 3.0, 108);
    return Math.min(16 + countBoost * 3.4 + lightBoost * 1.0, 92);
  }

  // 互动分数：count 权重高，light_sum 权重低
  function interactionScore(node) {
    return node.count * 3 + node.light_sum * 0.5;
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
    maxR = Math.max(120, Math.min(cx, cy) - 70);
  }

  // ── 数据构建 ──

  function buildGraphData(data) {
    totalTargets = data.total_targets || 0;

    // 主节点
    const mainRaw = data.nodes.find(n => n.is_main) || data.nodes[0];
    mainNode = {
      ...mainRaw,
      is_main: true,
      size: nodeSize(mainRaw),
      color: nodeColor(mainRaw),
      glow: nodeGlow(mainRaw),
      x: cx,
      y: cy,
      labelPos: 'bottom',
      isTop: false,
    };

    // 非主节点：计算分数、颜色、大小
    let others = data.nodes
      .filter(n => !n.is_main)
      .map(n => ({
        ...n,
        size: nodeSize(n),
        color: nodeColor(n),
        glow: nodeGlow(n),
        score: interactionScore(n),
      }));

    // Top 互动对象：前 3 默认显示，前 5 缩放后显示
    const sortedByCount = others
      .slice()
      .sort((a, b) => b.count - a.count);
    const top3Names = new Set(sortedByCount.slice(0, 3).map(n => n.name));
    const top5Names = new Set(sortedByCount.slice(0, 5).map(n => n.name));

    // 按分数降序：分数越高越靠中心
    others.sort((a, b) => b.score - a.score);

    // 按分数降序：分数越高越靠中心
    others.sort((a, b) => b.score - a.score);

    // 动态环数与容量：内环周长小放少，外环周长大放多；保证弧长间距
    const minPerRing = 4;
    const maxPerRing = 8;
    let remaining = others.slice();
    const groups = [];
    let ri = 0;
    const maxRings = 10;
    while (remaining.length > 0 && ri < maxRings) {
      // 先用临时半径估算容量
      const tmpFrac = 0.30 + 0.68 * (ri / Math.max(maxRings - 1, 1));
      const tmpR = maxR * tmpFrac;
      const capacity = Math.max(
        minPerRing,
        Math.min(maxPerRing, Math.floor((2 * Math.PI * tmpR) / 72)),
      );
      const group = remaining.slice(0, capacity);
      remaining = remaining.slice(capacity);
      if (!group.length) break;
      groups.push(group);
      ri++;
    }

    // 根据实际环数重新分配半径，确保外环真正靠近边界
    const actualRings = groups.length;
    rings = groups.map((group, ri) => {
      const frac = 0.30 + 0.68 * (ri / Math.max(actualRings - 1, 1));
      const r = maxR * frac;
      // 速度：内环快，外环慢，差异拉大；相邻环反向旋转，减少交叉重叠
      const baseSpeed = 0.22 - 0.18 * (ri / Math.max(actualRings - 1, 1));
      const direction = ri % 2 === 0 ? 1 : -1;
      // 相位错开，避免所有星球排成放射线
      const ringPhase = ri * 0.55;

      return {
        r,
        speed: baseSpeed,
        nodes: group.map((n, i) => {
          // 同一环内均匀分布 + 小幅随机扰动
          const jitter = (Math.random() - 0.5) * 0.35;
          const baseAngle = ringPhase + (i / Math.max(group.length, 1)) * Math.PI * 2 + jitter;
          // 每个星球速度 ±25% 扰动，让相对位置持续变化
          const speedVar = 0.75 + Math.random() * 0.5;
          return {
            ...n,
            r,
            baseAngle,
            speed: baseSpeed * speedVar * direction,
            x: cx + r * Math.cos(baseAngle),
            y: cy + r * Math.sin(baseAngle),
            labelPos: Math.cos(baseAngle) >= 0 ? 'right' : 'left',
            isTop: top5Names.has(n.name),
            isTop3: top3Names.has(n.name),
          };
        }),
      };
    });

    allNodes = [mainNode, ...rings.flatMap(r => r.nodes)];
  }

  // ── 动画循环 ──

  function updateNodePositions(elapsedSec) {
    // 主节点始终居中，轻微呼吸光晕在绘制时处理
    mainNode.x = cx;
    mainNode.y = cy;

    for (const ring of rings) {
      for (const n of ring.nodes) {
        const ang = n.baseAngle + elapsedSec * n.speed;
        n.x = cx + n.r * Math.cos(ang);
        n.y = cy + n.r * Math.sin(ang);
        n.labelPos = Math.cos(ang) >= 0 ? 'right' : 'left';
      }
    }

    // 按 y 坐标排序，实现遮挡关系（y 小的在后方）
    allNodes = [mainNode, ...rings.flatMap(r => r.nodes)];
    allNodes.sort((a, b) => a.y - b.y);
  }

  function drawOrbits() {
    const t = theme();
    ctx.save();
    ctx.strokeStyle = t.orbit;
    ctx.lineWidth = 1;
    for (const ring of rings) {
      ctx.beginPath();
      ctx.arc(cx, cy, ring.r, 0, Math.PI * 2);
      ctx.stroke();
    }
    ctx.restore();
  }

  function drawNode(node, elapsedSec) {
    const t = theme();
    const isMain = node.is_main;
    const size = isMain
      ? node.size * (1 + 0.045 * Math.sin(elapsedSec * 2.2))
      : node.size;

    ctx.save();

    // 光晕
    if (node.glow) {
      ctx.shadowBlur = isMain
        ? node.glow.blur * (1 + 0.25 * Math.sin(elapsedSec * 2.2))
        : node.glow.blur;
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
      grad.addColorStop(0.5, theme().accent);
      grad.addColorStop(1, '#7c3aed');
      ctx.fillStyle = grad;
    } else {
      ctx.fillStyle = node.color;
    }
    ctx.fill();

    // 边框
    ctx.shadowBlur = 0;
    ctx.strokeStyle = t.nodeBorder;
    ctx.lineWidth = isMain ? 2.5 : 1.5;
    ctx.stroke();

    // 主节点内的小高光
    if (isMain) {
      ctx.beginPath();
      ctx.arc(
        node.x - size * 0.18,
        node.y - size * 0.18,
        size * 0.12,
        0,
        Math.PI * 2,
      );
      ctx.fillStyle = 'rgba(255,255,255,0.22)';
      ctx.fill();
    }

    ctx.restore();
  }

  function drawLabels() {
    const t = theme();
    ctx.save();
    ctx.textBaseline = 'middle';

    // 默认只显示重要标签；缩放大时逐步显示更多；hover 的节点始终显示
    for (const node of allNodes) {
      const isMain = node.is_main;
      const isHovered = hoveredNode && hoveredNode.name === node.name;
      const shouldShow = isMain
        || isHovered
        || node.isTop3
        || node.size >= 42
        || (node.isTop && scale > 1.15)
        || scale > 1.35;

      if (!shouldShow) continue;

      const fontSize = isMain ? 13 : (node.isTop || isHovered ? 11.5 : 10);
      const fontWeight = isMain || node.isTop || isHovered ? 'bold' : '500';
      // 与 tailwind.config 的 fontFamily.sans 保持一致，保证全站字体统一
      ctx.font = `${fontWeight} ${fontSize}px "Segoe UI", "Helvetica Neue", "PingFang SC", "Microsoft YaHei", sans-serif`;

      const label = node.name;
      const metrics = ctx.measureText(label);
      const textH = fontSize + 4;
      const padX = 5;
      const padY = 2;
      const radius = node.size / 2 + 5;

      let tx, ty, align;
      if (isMain) {
        tx = node.x;
        ty = node.y + radius + 10;
        align = 'center';
      } else if (node.labelPos === 'right') {
        tx = node.x + radius;
        ty = node.y;
        align = 'left';
      } else {
        tx = node.x - radius;
        ty = node.y;
        align = 'right';
      }

      // 文字背景块（更小巧、更淡）
      const bgW = metrics.width + padX * 2;
      const bgH = textH + padY * 2;
      let bgX = tx - (align === 'center' ? bgW / 2 : align === 'left' ? 0 : bgW);
      let bgY = ty - bgH / 2;

      ctx.fillStyle = isHovered ? (store.darkMode.value ? 'rgba(28,28,30,0.7)' : 'rgba(255,255,255,0.75)') : t.bg;
      if (ctx.roundRect) {
        ctx.beginPath();
        ctx.roundRect(bgX, bgY, bgW, bgH, 5);
        ctx.fill();
      } else {
        ctx.fillRect(bgX, bgY, bgW, bgH);
      }

      // 文字描边
      ctx.strokeStyle = t.nodeBorder;
      ctx.lineWidth = 2;
      ctx.textAlign = align;
      ctx.strokeText(label, tx, ty);

      // 文字填充
      ctx.fillStyle = isMain ? t.accent : (isHovered ? t.text : t.subText);
      ctx.fillText(label, tx, ty);
    }

    ctx.restore();
  }

  function drawHover() {
    if (!hoveredNode) return;
    const t = theme();
    ctx.save();
    ctx.beginPath();
    ctx.arc(hoveredNode.x, hoveredNode.y, hoveredNode.size / 2 + 8, 0, Math.PI * 2);
    ctx.strokeStyle = t.accent;
    ctx.lineWidth = 2;
    ctx.setLineDash([4, 4]);
    ctx.stroke();
    ctx.restore();
  }

  function draw() {
    if (!ctx || !canvas) return;

    const t = theme();
    const elapsed = (performance.now() - startTime) / 1000;

    // 平滑视图变换
    scale += (targetScale - scale) * 0.12;
    panX += (targetPanX - panX) * 0.12;
    panY += (targetPanY - panY) * 0.12;

    updateNodePositions(elapsed);

    // 清空画布
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);

    // 应用视图变换
    ctx.setTransform(scale * dpr, 0, 0, scale * dpr, panX * dpr, panY * dpr);

    // 绘制轨道
    drawOrbits();

    // 绘制节点（已按 y 排序）
    for (const node of allNodes) {
      drawNode(node, elapsed);
    }

    // 绘制标签（最上层）
    drawLabels();

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
      const r = node.size / 2 + 6; // 稍微扩大命中区域
      const dx = p.x - node.x;
      const dy = p.y - node.y;
      const dist2 = dx * dx + dy * dy;
      // 优先命中 y 更大（更近）的节点；同距离时后遍历的覆盖前面的
      if (dist2 <= r * r && (dist2 < bestDist || (best && node.y > best.y))) {
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
    startTime = performance.now();
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

    // 保存旧视图，resize 时可恢复
    const prevScale = scale;
    const prevPanX = panX;
    const prevPanY = panY;

    clearGraph();
    containerEl = ensureCanvas();
    if (!containerEl) return;

    sizeCanvas();
    buildGraphData(store.graphData.value);

    // 恢复视图（首次渲染则使用默认值）
    if (prevScale !== 1 || prevPanX !== 0 || prevPanY !== 0) {
      scale = prevScale;
      panX = prevPanX;
      panY = prevPanY;
      targetScale = prevScale;
      targetPanX = prevPanX;
      targetPanY = prevPanY;
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

  // ── 数据加载与详情抽屉（与原接口保持一致） ──

  async function loadGraph() {
    const euid = store.euid.value.trim();
    if (!euid || store.graphLoading.value) return;
    store.graphLoading.value = true;
    store.graphError.value = '';
    store.graphDetail.value = null;
    try {
      const res = await fetch(`/api/interactions/graph?euid=${encodeURIComponent(euid)}`);
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
    store.graphData.value = null;
    store.graphError.value = '';
  }

  return {
    loadGraph,
    renderGraph,
    openDetail,
    closeDetail,
    loadMoreDetail,
    reset,
    nodeColor,
  };
}
