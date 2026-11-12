const finite = p => p && Number.isFinite(p.x) && Number.isFinite(p.y);
const size = (value, fallback) => Number.isFinite(value) && value > 0 ? value : fallback;

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
