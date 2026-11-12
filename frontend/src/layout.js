const finite = p => p && Number.isFinite(p.x) && Number.isFinite(p.y);
const size = (value, fallback) => Number.isFinite(value) && value > 0 ? value : fallback;
const gap = {column: 140, row: 60, child: 70, band: 120, block: 140};
const target = d => typeof d === 'string' ? d : d.target;

function relations(tasks) {
  const byId = new Map(tasks.map(t => [t.id, t])), parent = new Map(), flows = [], links = [];
  for (const t of tasks) {
    let p = t.parent;
    const seen = new Set([t.id]);
    while (byId.has(p) && !seen.has(p)) {seen.add(p); p = byId.get(p).parent;}
    if (byId.has(t.parent) && p !== t.id) parent.set(t.id, t.parent);
    for (const from of [...(t.dependencies || []).map(target), ...(t.after || [])]) if (byId.has(from) && from !== t.id) flows.push([from, t.id]);
    for (const other of [...(t.conflicts || []), ...(t.related || [])]) if (byId.has(other) && other !== t.id) links.push([other, t.id]);
  }
  const children = new Map(tasks.map(t => [t.id, []]));
  for (const [id, p] of parent) children.get(p).push(id);
  children.forEach(list => list.sort());
  const measure = measurements => id => {
    const t = byId.get(id), m = measurements[id];
    return {width: size(m?.width, 220), height: size(m?.height, 50 + Math.ceil(t.title.length / 24) * 20 + (children.get(id).length || t.container ? 80 : 0))};
  };
  return {byId, parent, children, flows, links, measure};
}

function score(width, height, room) {
  const fit = Math.min(room.width / Math.max(width, 1), room.height / Math.max(height, 1));
  return Math.min(fit, 1 / (room.scale || 1)) + fit * 1e-6;
}

export function arrange(tasks, measurements = {}, room = {width: 2000, height: 1000}) {
  const {byId, parent, children, flows, links, measure} = relations(tasks), sizeOf = measure(measurements), positions = {};
  function block(id) {
    const own = sizeOf(id), kids = children.get(id);
    if (!kids.length) return {width: own.width, height: own.height, place: (x, y) => {positions[id] = {x, y};}};
    const group = layout(kids, false), width = Math.max(own.width, group.width);
    return {width, height: own.height + gap.child + group.height, place: (x, y) => {
      positions[id] = {x: x + (width - own.width) / 2, y};
      group.place(x + (width - group.width) / 2, y + own.height + gap.child);
    }};
  }
  function layout(items, banded) {
    const owner = new Map(), claim = (id, item) => {owner.set(id, item); children.get(id).forEach(child => claim(child, item));};
    items.forEach(item => claim(item, item));
    const before = new Map(items.map(item => [item, new Set()])), near = new Map(items.map(item => [item, new Set()]));
    for (const [kind, list] of [['flow', flows], ['link', links]]) for (const [a, b] of list) {
      const from = owner.get(a), to = owner.get(b);
      if (!from || !to || from === to) continue;
      if (kind === 'flow') before.get(to).add(from);
      near.get(from).add(to); near.get(to).add(from);
    }
    const rank = new Map(), trail = new Set();
    const rankOf = item => {
      if (rank.has(item)) return rank.get(item);
      if (trail.has(item)) return 0;
      trail.add(item);
      const value = Math.max(0, ...[...before.get(item)].map(p => rankOf(p) + 1));
      trail.delete(item); rank.set(item, value);
      return value;
    };
    items.forEach(rankOf);
    const columns = [...new Set(items.map(item => rank.get(item)))].sort((a, b) => a - b).map(r => items.filter(item => rank.get(item) === r));
    const order = new Map();
    const index = () => columns.forEach(column => column.forEach((item, i) => order.set(item, i / Math.max(1, column.length - 1))));
    index();
    for (let pass = 0; pass < 4; pass++) {
      for (const column of pass % 2 ? [...columns].reverse() : columns) {
        const keys = new Map(column.map(item => {
          const others = [...near.get(item)].filter(other => rank.get(other) !== rank.get(item));
          return [item, others.length ? others.reduce((sum, other) => sum + order.get(other), 0) / others.length : order.get(item)];
        }));
        column.sort((a, b) => keys.get(a) - keys.get(b));
        column.forEach((item, i) => order.set(item, i / Math.max(1, column.length - 1)));
      }
    }
    const blocks = new Map(items.map(item => [item, block(item)]));
    const tallest = Math.max(...columns.map(column => column.length));
    let best;
    for (let rows = tallest; rows >= 1; rows--) {
      const subs = columns.flatMap(column => {
        const count = Math.ceil(column.length / rows), per = Math.ceil(column.length / count);
        return Array.from({length: count}, (_, i) => column.slice(i * per, (i + 1) * per));
      }).map(list => ({list, width: Math.max(...list.map(item => blocks.get(item).width)), height: list.reduce((sum, item) => sum + blocks.get(item).height, 0) + (list.length - 1) * gap.row}));
      const total = subs.reduce((sum, sub) => sum + sub.width + gap.column, -gap.column), high = Math.max(...subs.map(sub => sub.height));
      const guess = Math.sqrt(total * room.height / (room.width * high));
      const counts = banded || total > room.width ? [...new Set([1, Math.floor(guess), Math.ceil(guess), Math.ceil(guess) + 1])].filter(k => k >= 1 && k <= subs.length) : [1];
      for (const k of counts) {
        const bands = Array.from({length: k}, () => []);
        let start = 0;
        for (const sub of subs) {bands[Math.min(k - 1, Math.floor((start + sub.width / 2) / (total / k)))].push(sub); start += sub.width + gap.column;}
        const shaped = bands.filter(band => band.length).map(band => ({band, width: band.reduce((sum, sub) => sum + sub.width + gap.column, -gap.column), height: Math.max(...band.map(sub => sub.height))}));
        const width = Math.max(...shaped.map(b => b.width)), height = shaped.reduce((sum, b) => sum + b.height + gap.band, -gap.band);
        const value = score(width, height, room) * 0.9 ** (shaped.length - 1) * (rows < tallest ? 0.75 : 1);
        if (!best || value > best.value) best = {value, width, height, shaped};
      }
    }
    return {width: best.width, height: best.height, place: (x, y) => {
      let top = y;
      for (const {band, height} of best.shaped) {
        let left = x;
        for (const sub of band) {
          let row = top;
          for (const item of sub.list) {const b = blocks.get(item); b.place(left + (sub.width - b.width) / 2, row); row += b.height + gap.row;}
          left += sub.width + gap.column;
        }
        top += height + gap.band;
      }
    }};
  }
  const group = new Map(tasks.map(t => [t.id, t.id]));
  const find = id => group.get(id) === id ? id : (group.set(id, find(group.get(id))), group.get(id));
  for (const [a, b] of [...flows, ...links, ...parent]) group.set(find(a), find(b));
  const components = new Map();
  for (const id of [...byId.keys()].sort()) {const root = find(id); components.set(root, [...(components.get(root) || []), id]);}
  const pieces = [...components.values()].map(members => {
    const roots = members.filter(id => !parent.has(id));
    return {members, ...(roots.length === 1 ? block(roots[0]) : layout(roots, true))};
  }).sort((a, b) => b.height - a.height || b.members.length - a.members.length || a.members[0].localeCompare(b.members[0]));
  let best;
  for (let limit = 0, i = 0; i < pieces.length; i++) {
    limit += pieces[i].width + (i ? gap.block : 0);
    const rows = [];
    for (const piece of pieces) {
      const row = rows[rows.length - 1];
      if (row && row.width + gap.block + piece.width <= limit) {row.list.push(piece); row.width += gap.block + piece.width; row.height = Math.max(row.height, piece.height);}
      else rows.push({list: [piece], width: piece.width, height: piece.height});
    }
    const width = Math.max(...rows.map(row => row.width)), height = rows.reduce((sum, row) => sum + row.height + gap.band, -gap.band);
    const value = score(width, height, room);
    if (!best || value > best.value) best = {value, rows};
  }
  let top = 0;
  for (const row of best?.rows || []) {
    let left = 0;
    for (const piece of row.list) {piece.place(left, top); left += piece.width + gap.block;}
    top += row.height + gap.band;
  }
  return Object.fromEntries(Object.entries(positions).map(([id, p]) => [id, {x: Math.round(p.x), y: Math.round(p.y)}]));
}

export function extend(tasks, positions, measurements = {}) {
  const missing = tasks.filter(t => !finite(positions[t.id]));
  if (!missing.length) return positions;
  const {byId, parent, children, flows, links, measure} = relations(tasks), sizeOf = measure(measurements), result = {...positions};
  const boxes = tasks.filter(t => finite(result[t.id])).map(t => ({...result[t.id], ...sizeOf(t.id)}));
  const box = id => finite(result[id]) && byId.has(id) ? {...result[id], ...sizeOf(id)} : null;
  const mean = list => list.reduce((sum, value) => sum + value, 0) / list.length;
  const anchor = id => {
    const own = sizeOf(id);
    const up = box(parent.get(id));
    if (up) return {x: up.x, y: up.y + up.height + gap.child};
    const from = flows.filter(([, b]) => b === id).map(([a]) => box(a)).filter(Boolean);
    if (from.length) return {x: Math.max(...from.map(b => b.x + b.width)) + gap.column, y: mean(from.map(b => b.y))};
    const to = [...flows.filter(([a]) => a === id).map(([, b]) => b), ...children.get(id)].map(box).filter(Boolean);
    if (to.length) return {x: Math.min(...to.map(b => b.x)) - gap.column - own.width, y: mean(to.map(b => b.y))};
    const side = links.filter(pair => pair.includes(id)).map(pair => box(pair[0] === id ? pair[1] : pair[0])).filter(Boolean);
    if (side.length) return {x: Math.max(...side.map(b => b.x + b.width)) + gap.column, y: mean(side.map(b => b.y))};
    if (!boxes.length) return {x: 0, y: 0};
    return {x: Math.min(...boxes.map(b => b.x)), y: Math.max(...boxes.map(b => b.y + b.height)) + gap.band};
  };
  const free = (p, own) => boxes.every(b => p.x + own.width + gap.row <= b.x || b.x + b.width + gap.row <= p.x || p.y + own.height + gap.row <= b.y || b.y + b.height + gap.row <= p.y);
  const linked = id => [parent.get(id), ...children.get(id), ...flows.concat(links).filter(pair => pair.includes(id)).flat()].filter(other => other && other !== id && finite(result[other])).length;
  const queue = [...missing];
  while (queue.length) {
    queue.sort((a, b) => linked(b.id) - linked(a.id) || a.id.localeCompare(b.id));
    const task = queue.shift(), own = sizeOf(task.id), start = anchor(task.id);
    let spot;
    for (let ring = 0; !spot; ring++) {
      const options = [];
      for (let i = -ring; i <= ring; i++) for (let j = -ring; j <= ring; j++) if (Math.max(Math.abs(i), Math.abs(j)) === ring) options.push({x: start.x + i * 60, y: start.y + j * 40});
      spot = options.filter(p => free(p, own)).sort((a, b) => Math.hypot(a.x - start.x, (a.y - start.y) * 1.5) - Math.hypot(b.x - start.x, (b.y - start.y) * 1.5))[0];
    }
    result[task.id] = {x: Math.round(spot.x), y: Math.round(spot.y)};
    boxes.push({...result[task.id], ...own});
  }
  return result;
}

export function createSimulation(tasks, positions = {}, dimensions = {}) {
  let nodes = new Map(), links = [], alpha = 1, cancelled = false;
  const pinned = new Set();
  const snapshot = () => Object.fromEntries([...nodes].map(([id, n]) => [id, {x: n.x, y: n.y}]));
  function update(tasks, positions = {}, dimensions = {}) {
    const next = new Map();
    for (const task of [...tasks].sort((a, b) => a.id.localeCompare(b.id))) {
      const previous = nodes.get(task.id);
      const p = previous || (finite(positions[task.id]) ? positions[task.id] : {x: 0, y: 0});
      next.set(task.id, {x: p.x, y: p.y, width: size(dimensions[task.id]?.width, previous?.width || 220), height: size(dimensions[task.id]?.height, previous?.height || 180)});
    }
    nodes = next;
    links = [];
    const seen = new Set();
    for (const task of tasks) {
      const add = (id, kind) => {
        if (!nodes.has(id) || id === task.id) return;
        const key = JSON.stringify([kind, ...(['conflicts', 'related'].includes(kind) ? [id, task.id].sort() : [id, task.id])]);
        if (!seen.has(key)) {seen.add(key); links.push({source: id, target: task.id, kind});}
      };
      for (const d of task.dependencies || []) add(typeof d === 'string' ? d : d.target, 'dependency');
      for (const kind of ['after', 'conflicts', 'related']) for (const id of task[kind] || []) add(id, kind);
      if (task.parent) add(task.parent, 'parent');
    }
    links.sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b)));
    alpha = 1;
  }
  function step() {
    if (cancelled) return {positions: snapshot(), done: true};
    const list = [...nodes], before = snapshot(), forces = new Map(list.map(([id]) => [id, {x: 0, y: 0}]));
    const move = (id, x, y) => {if (!pinned.has(id)) {const n = nodes.get(id); n.x += x; n.y += y;}};
    const pair = (a, b) => {
      let x = b.x + b.width / 2 - a.x - a.width / 2, y = b.y + b.height / 2 - a.y - a.height / 2;
      if (!x && !y) {x = 1; y = 0.5;}
      const distance = Math.hypot(x, y);
      return {x: x / distance, y: y / distance, distance};
    };
    const force = (a, b, x, y) => {forces.get(a).x += x; forces.get(a).y += y; forces.get(b).x -= x; forces.get(b).y -= y;};
    for (let i = 0; i < list.length; i++) for (let j = i + 1; j < list.length; j++) {
      const [a, an] = list[i], [b, bn] = list[j], p = pair(an, bn);
      const scale = (an.width + bn.width + an.height + bn.height) / 4;
      const strength = scale * scale / (p.distance + scale) * 0.12;
      force(a, b, -p.x * strength, -p.y * strength);
    }
    for (const {source, target} of links) {
      const a = nodes.get(source), b = nodes.get(target), p = pair(a, b);
      const rest = Math.hypot((a.width + b.width) / 2, (a.height + b.height) / 2) + 60;
      const strength = (p.distance - rest) * 0.08;
      force(source, target, p.x * strength, p.y * strength);
    }
    for (const [id] of list) {const f = forces.get(id); move(id, f.x * alpha / Math.max(1, links.length / Math.max(1, nodes.size)), f.y * alpha / Math.max(1, links.length / Math.max(1, nodes.size)));}
    let overlap = 0;
    for (let i = 0; i < list.length; i++) for (let j = i + 1; j < list.length; j++) {
      const [a, an] = list[i], [b, bn] = list[j];
      const dx = bn.x + bn.width / 2 - an.x - an.width / 2, dy = bn.y + bn.height / 2 - an.y - an.height / 2;
      const ox = (an.width + bn.width) / 2 + 24 - Math.abs(dx), oy = (an.height + bn.height) / 2 + 24 - Math.abs(dy);
      const movable = Number(!pinned.has(a)) + Number(!pinned.has(b));
      if (ox <= 0 || oy <= 0 || !movable) continue;
      overlap = Math.max(overlap, Math.min(ox, oy));
      const x = ox < oy ? (dx < 0 ? -ox : ox) / movable : 0;
      const y = ox < oy ? 0 : (dy < 0 ? -oy : oy) / movable;
      move(a, -x, -y); move(b, x, y);
    }
    alpha *= 0.96;
    const movement = Math.max(0, ...list.map(([id, n]) => Math.hypot(n.x - before[id].x, n.y - before[id].y)));
    return {positions: snapshot(), done: alpha < 0.001 && movement < 0.05 && overlap < 0.05};
  }
  update(tasks, positions, dimensions);
  return {step, update, snapshot, cancel() {cancelled = true;}, pin(id, position) {if (nodes.has(id) && finite(position)) {Object.assign(nodes.get(id), position); pinned.add(id);}}, unpin(id) {pinned.delete(id); alpha = 1;}, get links() {return links.map(link => ({...link}));}};
}
