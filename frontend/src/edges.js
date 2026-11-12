import {connection} from './routing';

const lanes = {dependency: 0, after: 26, conflict: -26, related: -52};
const inside = (p, r, pad) => p.x > r.x - pad && p.x < r.x + r.width + pad && p.y > r.y - pad && p.y < r.y + r.height + pad;
const cubic = (a, b, c, d) => t => {
  const u = 1 - t;
  return {x: u * u * u * a.x + 3 * u * u * t * b.x + 3 * u * t * t * c.x + t * t * t * d.x, y: u * u * u * a.y + 3 * u * u * t * b.y + 3 * u * t * t * c.y + t * t * t * d.y};
};
const sample = (f, n) => Array.from({length: n + 1}, (_, i) => f(i / n));
const svg = (a, b, c, d) => `M ${a.x},${a.y} C ${b.x},${b.y} ${c.x},${c.y} ${d.x},${d.y}`;

function bestCurve(start, end, rectangles, controls, bows) {
  let best;
  for (const bow of bows) {
    const [b, c] = controls(bow), points = sample(cubic(start, b, c, end), 28);
    const blocked = points.slice(2, -2).filter(p => rectangles.some(r => inside(p, r, 8))).length;
    if (!best || blocked < best.blocked) best = {blocked, points, path: svg(start, b, c, end)};
    if (!blocked) break;
  }
  return best;
}

function rounded(points) {
  const parts = [`M ${points[0].x},${points[0].y}`];
  points.slice(1, -1).forEach((p, i) => {
    const a = points[i], b = points[i + 2], la = Math.hypot(a.x - p.x, a.y - p.y), lb = Math.hypot(b.x - p.x, b.y - p.y);
    const r = Math.min(20, la / 2, lb / 2);
    if (!r) return;
    parts.push(`L ${p.x + (a.x - p.x) * r / la},${p.y + (a.y - p.y) * r / la}`, `Q ${p.x},${p.y} ${p.x + (b.x - p.x) * r / lb},${p.y + (b.y - p.y) * r / lb}`);
  });
  const last = points[points.length - 1];
  parts.push(`L ${last.x},${last.y}`);
  return parts.join(' ');
}

const side = (r, sign, y) => ({x: sign > 0 ? r.x + r.width : r.x, y: y ?? r.y + r.height / 2});

function relationGeometry(kind, source, destination, rectangles, ports) {
  const lane = lanes[kind] || 0, step = Math.max(60, Math.min(source.height, destination.height) / 2 + 30);
  const ks = [0, 1, -1, 2, -2, 3, -3, 5, -5];
  const ahead = destination.x - source.x - source.width, behind = source.x - destination.x - destination.width;
  if (ahead > 30 || behind > 30) {
    const sign = ahead > 30 ? 1 : -1, start = side(source, sign, ports.out), end = side(destination, -sign, ports.in);
    const reach = Math.max(50, Math.abs(end.x - start.x) / 2);
    const curve = bestCurve(start, end, rectangles, bow => [{x: start.x + sign * reach, y: start.y + bow}, {x: end.x - sign * reach, y: end.y + bow}], ks.map(k => lane + k * step));
    return {...curve, start, end};
  }
  const gap = Math.abs(destination.y + destination.height / 2 - source.y - source.height / 2);
  const right = Math.max(source.x + source.width, destination.x + destination.width), left = Math.min(source.x, destination.x);
  const candidates = [0, 1, 2, 3, 5].flatMap(k => [1, -1].map(sign => ({sign, reach: 50 + gap / 4 + Math.abs(lane) + k * step})));
  let best;
  for (const {sign, reach} of candidates) {
    const start = side(source, sign, sign > 0 ? ports.out : undefined), end = side(destination, sign, sign > 0 ? ports.in : undefined), edge = sign > 0 ? right : left;
    const curve = bestCurve(start, end, rectangles, () => [{x: edge + sign * reach, y: start.y}, {x: edge + sign * reach, y: end.y}], [0]);
    if (!best || curve.blocked < best.blocked) best = {...curve, start, end};
    if (!curve.blocked) break;
  }
  return best;
}

const facing = (source, destination) => destination.x - source.x - source.width > 30 ? [1, -1] : source.x - destination.x - destination.width > 30 ? [-1, 1] : [1, 1];

export function relationLayout(list, rectangles) {
  const groups = new Map(), ports = new Map(list.map(e => [e.id, {}]));
  const middle = r => r.y + r.height / 2;
  const join = (rect, sign, entry) => {
    const key = `${rect.id}:${sign}`;
    if (!groups.has(key)) groups.set(key, {rect, entries: []});
    groups.get(key).entries.push(entry);
  };
  for (const e of list) {
    const [a, b] = facing(e.source, e.destination);
    join(e.source, a, {id: e.id, end: 'out', order: middle(e.destination)});
    join(e.destination, b, {id: e.id, end: 'in', order: middle(e.source)});
  }
  for (const {rect, entries} of groups.values()) {
    entries.sort((p, q) => p.order - q.order || (p.id < q.id ? -1 : 1));
    entries.forEach((entry, i) => {ports.get(entry.id)[entry.end] = rect.y + rect.height * (0.2 + 0.6 * (i + 1) / (entries.length + 1));});
  }
  return new Map(list.map(e => [e.id, relationGeometry(e.kind, e.source, e.destination, rectangles, ports.get(e.id))]));
}
export function hierarchyGeometry(source, destination, port, rectangles) {
  const {index, count} = port;
  const exit = {x: source.x + source.width * (index + 1) / (count + 1), y: source.y + source.height}, entry = {x: destination.x + destination.width * (destination.parent ? 0.75 : 0.5), y: destination.y};
  const bend = Math.max(40, (entry.y - exit.y) / 2);
  const others = rectangles.filter(r => r !== source && r !== destination);
  const curve = bestCurve(exit, entry, others, () => [{x: exit.x, y: exit.y + bend}, {x: entry.x, y: entry.y - bend}], [0]);
  if (entry.y > exit.y + 20 && !curve.blocked) return {...curve, start: exit, end: entry};
  const lane = 10 + 18 * index / Math.max(1, count - 1);
  const routed = connection(exit, entry, rectangles, lane, [{x: exit.x, y: exit.y + lane}, {x: entry.x, y: entry.y - lane}]);
  const points = routed.length ? routed : [exit, entry];
  return {points, path: rounded(points), start: exit, end: entry};
}

function along(points, fraction) {
  const lengths = points.slice(1).map((p, i) => Math.hypot(p.x - points[i].x, p.y - points[i].y));
  let left = lengths.reduce((a, b) => a + b, 0) * fraction, i = 0;
  while (i < lengths.length - 1 && left > lengths[i]) left -= lengths[i++];
  const a = points[i], b = points[i + 1], t = lengths[i] ? left / lengths[i] : 0;
  return {x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, dx: b.x - a.x, dy: b.y - a.y};
}

function local(box, p) {
  const x = p.x - box.cx, y = p.y - box.cy;
  return {x: x * box.cos + y * box.sin, y: -x * box.sin + y * box.cos};
}

function cuts(box, a, b) {
  const p = local(box, a), q = local(box, b), d = {x: q.x - p.x, y: q.y - p.y};
  let lo = 0, hi = 1;
  for (const [pp, qq] of [[-d.x, p.x + box.w], [d.x, box.w - p.x], [-d.y, p.y + box.h], [d.y, box.h - p.y]]) {
    if (!pp) { if (qq < 0) return false; continue; }
    const t = qq / pp;
    if (pp < 0) lo = Math.max(lo, t); else hi = Math.min(hi, t);
    if (lo > hi) return false;
  }
  return true;
}

function corners(box) {
  return [[-1, -1], [1, -1], [1, 1], [-1, 1]].map(([sx, sy]) => ({x: box.cx + sx * box.w * box.cos - sy * box.h * box.sin, y: box.cy + sx * box.w * box.sin + sy * box.h * box.cos}));
}

function overlaps(box, other) {
  const mine = corners(box), theirs = corners(other);
  return mine.some((a, i) => theirs.some((c, j) => {
    const b = mine[(i + 1) % 4], d = theirs[(j + 1) % 4];
    const cross = (o, p, q) => (p.x - o.x) * (q.y - o.y) - (p.y - o.y) * (q.x - o.x);
    return cross(a, b, c) * cross(a, b, d) <= 0 && cross(c, d, a) * cross(c, d, b) <= 0;
  })) || contains(box, {x: other.cx, y: other.cy}) || contains(other, {x: box.cx, y: box.cy});
}

function contains(box, p) {
  const q = local(box, p);
  return Math.abs(q.x) < box.w && Math.abs(q.y) < box.h;
}

function distance(p, points) {
  let best = Infinity;
  for (let i = 1; i < points.length; i++) {
    const a = points[i - 1], b = points[i], dx = b.x - a.x, dy = b.y - a.y, l = dx * dx + dy * dy;
    const t = l ? Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / l)) : 0;
    best = Math.min(best, Math.hypot(p.x - a.x - dx * t, p.y - a.y - dy * t));
  }
  return best;
}

function extent(points, pad) {
  const xs = points.map(p => p.x), ys = points.map(p => p.y);
  return {x0: Math.min(...xs) - pad, y0: Math.min(...ys) - pad, x1: Math.max(...xs) + pad, y1: Math.max(...ys) + pad};
}

const near = (a, b) => a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1;

function slots(item, others, nodeBoxes) {
  const words = item.text.split(' '), variants = words.length > 1 ? [item.text, words.slice(1).join(' ')] : [item.text];
  const height = 14, lift = height / 2 + 1, result = [];
  const reach = extent(item.points, item.text.length * 6.6 + 40), nearby = others.filter(o => near(reach, o.extent));
  variants.forEach((label, short) => {
    const width = label.length * 6.6 + 4;
    for (let k = 0; k <= 10; k++) for (const flip of [-1, 1]) for (const level of [false, true]) for (const raised of [1, 0]) {
      const fraction = 0.25 + k * 0.05, p = along(item.points, fraction);
      let angle = level ? 0 : Math.atan2(p.dy, p.dx);
      if (angle > Math.PI / 2) angle -= Math.PI; else if (angle < -Math.PI / 2) angle += Math.PI;
      const cos = Math.cos(angle), sin = Math.sin(angle);
      const box = {cx: p.x + sin * flip * lift * raised, cy: p.y - cos * flip * lift * raised, w: width / 2, h: height / 2, cos, sin};
      const centre = {x: box.cx, y: box.cy}, own = distance(centre, item.points);
      const foreign = Math.min(Infinity, ...nearby.map(o => distance(centre, o.points)));
      const padded = {...box, w: box.w + 3, h: box.h + 3};
      if (foreign < 2 * own + 2 || nodeBoxes.some(n => overlaps(box, n)) || nearby.some(o => o.points.slice(1).some((q, i) => cuts(padded, o.points[i], q)))) continue;
      result.push({box, angle, label, slot: [short, k, flip, level, raised].join(), score: short * 1000 + (raised ? 0 : 500) + (level ? 200 : 0) + Math.round(Math.abs(fraction - 0.5) * 20) * 2 + (flip > 0 ? 1 : 0)});
    }
  });
  return result.sort((a, b) => a.score - b.score);
}

export function placeLabels(items, rectangles, memory = new Map()) {
  const placed = new Map(), boxes = [];
  const nodeBoxes = rectangles.map(r => ({cx: r.x + r.width / 2, cy: r.y + r.height / 2, w: r.width / 2 + 3, h: r.height / 2 + 3, cos: 1, sin: 0}));
  const all = items.filter(i => i.points?.length > 1).map(i => ({...i, extent: extent(i.points, 0)}));
  const free = s => !boxes.some(b => overlaps({...s.box, w: s.box.w + 2, h: s.box.h + 2}, b));
  const candidates = all.filter(i => i.text).sort((a, b) => (a.id < b.id ? -1 : 1)).map(item => ({item, options: slots(item, all.filter(o => o !== item), nodeBoxes)}));
  const kept = new Map(candidates.map(({item, options}) => [item.id, options.find(s => s.slot === memory.get(item.id))]));
  const order = [...candidates.filter(c => kept.get(c.item.id)), ...candidates.filter(c => !kept.get(c.item.id))];
  memory.clear();
  for (const {item, options} of order) {
    const previous = kept.get(item.id), choice = previous && free(previous) ? previous : options.find(free);
    if (!choice) continue;
    memory.set(item.id, choice.slot);
    boxes.push(choice.box);
    const {box} = choice, half = Math.abs(box.w * box.cos) + Math.abs(box.h * box.sin), tall = Math.abs(box.w * box.sin) + Math.abs(box.h * box.cos);
    placed.set(item.id, {x: box.cx, y: box.cy, label: choice.label, angle: choice.angle * 180 / Math.PI, bounds: {x0: box.cx - half, y0: box.cy - tall, x1: box.cx + half, y1: box.cy + tall}});
  }
  return placed;
}