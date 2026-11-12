function push(heap, item) {
  heap.push(item);
  for (let i = heap.length - 1; i > 0;) {
    const up = (i - 1) >> 1;
    if (heap[up][1] <= heap[i][1]) break;
    [heap[up], heap[i]] = [heap[i], heap[up]]; i = up;
  }
}
function pop(heap) {
  const top = heap[0], last = heap.pop();
  if (heap.length) {
    heap[0] = last;
    for (let i = 0; ;) {
      const l = 2 * i + 1, r = l + 1;
      let m = i;
      if (l < heap.length && heap[l][1] < heap[m][1]) m = l;
      if (r < heap.length && heap[r][1] < heap[m][1]) m = r;
      if (m === i) break;
      [heap[m], heap[i]] = [heap[i], heap[m]]; i = m;
    }
  }
  return top;
}
export function route(start, end, rectangles, clearance) {
  const boxes = rectangles.map(r => ({left: r.x - clearance, right: r.x + r.width + clearance, top: r.y - clearance, bottom: r.y + r.height + clearance}));
  const xs = [...new Set([start.x, end.x, ...boxes.flatMap(b => [b.left, b.right])])].sort((a, b) => a - b);
  const ys = [...new Set([start.y, end.y, ...boxes.flatMap(b => [b.top, b.bottom])])].sort((a, b) => a - b);
  const clear = (a, b) => !boxes.some(r => a.x === b.x ? a.x > r.left && a.x < r.right && Math.max(a.y, b.y) > r.top && Math.min(a.y, b.y) < r.bottom : a.y > r.top && a.y < r.bottom && Math.max(a.x, b.x) > r.left && Math.min(a.x, b.x) < r.right);
  const column = new Map(xs.map((x, i) => [x, i])), row = new Map(ys.map((y, i) => [y, i]));
  const index = p => row.get(p.y) * xs.length + column.get(p.x);
  const at = i => ({x: xs[i % xs.length], y: ys[Math.floor(i / xs.length)]});
  const first = index(start), last = index(end), costs = new Map([[first, 0]]), previous = new Map(), queue = [[first, 0]];
  while (queue.length) {
    const [id, cost] = pop(queue);
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
      if (distance < (costs.get(next) ?? Infinity) && clear(a, b)) { costs.set(next, distance); previous.set(next, id); push(queue, [next, distance]); }
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

