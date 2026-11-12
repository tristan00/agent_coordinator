export function route(start, end, rectangles, clearance) {
  const boxes = rectangles.map(r => ({left: r.x - clearance, right: r.x + r.width + clearance, top: r.y - clearance, bottom: r.y + r.height + clearance}));
  const xs = [...new Set([start.x, end.x, ...boxes.flatMap(b => [b.left, b.right])])].sort((a, b) => a - b);
  const ys = [...new Set([start.y, end.y, ...boxes.flatMap(b => [b.top, b.bottom])])].sort((a, b) => a - b);
  const clear = (a, b) => !boxes.some(r => a.x === b.x ? a.x > r.left && a.x < r.right && Math.max(a.y, b.y) > r.top && Math.min(a.y, b.y) < r.bottom : a.y > r.top && a.y < r.bottom && Math.max(a.x, b.x) > r.left && Math.min(a.x, b.x) < r.right);
  const index = p => ys.indexOf(p.y) * xs.length + xs.indexOf(p.x);
  const at = i => ({x: xs[i % xs.length], y: ys[Math.floor(i / xs.length)]});
  const first = index(start), last = index(end), costs = new Map([[first, 0]]), previous = new Map(), queue = [[first, 0]];
  while (queue.length) {
    queue.sort((a, b) => b[1] - a[1]);
    const [id, cost] = queue.pop();
    if (cost !== costs.get(id)) continue;
    if (id === last) {
      const points = [end];
      let cursor = last;
      while (cursor !== first) { cursor = previous.get(cursor); points.unshift(at(cursor)); }
      return points.filter((p, i) => !i || i === points.length - 1 || !((points[i - 1].x === p.x && points[i + 1].x === p.x) || (points[i - 1].y === p.y && points[i + 1].y === p.y)));
    }
    const x = id % xs.length, y = Math.floor(id / xs.length), a = at(id);
    for (const next of [x > 0 ? id - 1 : -1, x + 1 < xs.length ? id + 1 : -1, y > 0 ? id - xs.length : -1, y + 1 < ys.length ? id + xs.length : -1]) {
      if (next < 0) continue;
      const b = at(next), distance = cost + Math.abs(a.x - b.x) + Math.abs(a.y - b.y);
      if (distance < (costs.get(next) ?? Infinity) && clear(a, b)) { costs.set(next, distance); previous.set(next, id); queue.push([next, distance]); }
    }
  }
  return null;
}

export function connection(start, end, rectangles, clearance, waypoints) {
  const crosses = (a, b, r) => a.x === b.x
    ? a.x > r.x && a.x < r.x + r.width && Math.max(a.y, b.y) > r.y && Math.min(a.y, b.y) < r.y + r.height
    : a.y > r.y && a.y < r.y + r.height && Math.max(a.x, b.x) > r.x && Math.min(a.x, b.x) < r.x + r.width;
  const sections = waypoints.slice(1).map((p, i) => route(waypoints[i], p, rectangles, clearance));
  if (sections.every(Boolean)) {
    const points = [start, ...sections.flat(), end];
    if (points.slice(1).every((p, i) => !rectangles.some(r => crosses(points[i], p, r)))) return points;
  }
  const distance = (p, r) => Math.max(r.x - p.x, p.x - r.x - r.width, r.y - p.y, p.y - r.y - r.height, 0);
  const padding = Math.min(clearance, ...rectangles.flatMap(r => [distance(start, r), distance(end, r)]));
  return route(start, end, rectangles, padding) || route(start, end, rectangles, 0) || [];
}

export function arrange(tasks, measurements = {}) {
  const byId = new Map(tasks.map(t => [t.id, t])), depths = new Map(), placed = new Set(), positions = {};
  function depth(id, trail = new Set()) {
    if (depths.has(id)) return depths.get(id);
    if (trail.has(id)) return 0;
    const t = byId.get(id), next = new Set([...trail, id]);
    const value = Math.max(0, ...[...t.dependencies.map(d => typeof d === 'string' ? d : d.target), ...t.after].filter(p => byId.has(p)).map(p => depth(p, next) + 1));
    depths.set(id, value); return value;
  }
  const levels = new Map();
  function level(id, trail = new Set()) {
    if (levels.has(id)) return levels.get(id);
    if (trail.has(id)) return 0;
    const parent = byId.get(id).parent;
    const value = byId.has(parent) ? level(parent, new Set([...trail, id])) + 1 : 0;
    levels.set(id, value); return value;
  }
  tasks.forEach(t => level(t.id));
  const last = Math.max(0, ...levels.values()) + 1;
  tasks.filter(t => !byId.has(t.parent) && !tasks.some(child => child.parent === t.id)).forEach(t => levels.set(t.id, last));
  const heights = new Map();
  tasks.forEach(t => heights.set(levels.get(t.id), Math.max(heights.get(levels.get(t.id)) || 0, measurements[t.id]?.height || 160 + Math.ceil(t.title.length / 22) * 20)));
  const rows = new Map();
  let y = 0;
  for (const row of [...heights.keys()].sort((a, b) => a - b)) {rows.set(row, y); y += heights.get(row) + 110;}
  function place(task) {
    if (placed.has(task.id)) return;
    placed.add(task.id);
    const children = tasks.filter(t => t.parent === task.id && !placed.has(t.id));
    children.forEach(place);
    let x = children.length ? children.reduce((sum, t) => sum + positions[t.id].x, 0) / children.length : depth(task.id) * 380;
    const row = rows.get(levels.get(task.id));
    while (Object.values(positions).some(p => p.y === row && Math.abs(p.x - x) < 300)) x += 380;
    positions[task.id] = {x, y: row};
  }
  tasks.filter(t => !byId.has(t.parent)).forEach(place);
  tasks.forEach(place);
  return positions;
}
