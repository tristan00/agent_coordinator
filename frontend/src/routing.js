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

