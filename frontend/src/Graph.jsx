import React, {useEffect, useMemo, useRef, useState} from 'react';
import {ReactFlow, Background, Controls, MiniMap, MarkerType, Position, Handle, BaseEdge} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import {connection} from './routing';
import {createSimulation, arrange, extend} from './layout';
import {BorderBeam} from './Activity';

const views = new Map();
const legible = 0.6, margin = 24;
const target = d => typeof d === 'string' ? d : d.target;
const point = p => p && Number.isFinite(p.x) && Number.isFinite(p.y);
function readView(key) {
  if (views.has(key)) return views.get(key);
  let saved;
  try { saved = JSON.parse(localStorage.getItem(key)); } catch {}
  const positions = Object.fromEntries(Object.entries(saved?.positions || {}).filter(([, p]) => point(p)));
  const viewport = point(saved?.viewport) && Number.isFinite(saved.viewport.zoom) && saved.viewport.zoom > 0 ? saved.viewport : undefined;
  return {positions, viewport};
}
function TaskNode({data}) {
  return <>
    {data.active && <BorderBeam />}
    <Handle type="target" position={Position.Left} id="relations-in" />
    <Handle type="source" position={Position.Right} id="relations-out" />
    <Handle type="target" position={Position.Top} id="hierarchy-in" className="hierarchy-port" />
    <Handle type="source" position={Position.Bottom} id="hierarchy-out" className="hierarchy-port" />
    {data.label}
  </>;
}
const nodeTypes = {task: TaskNode};
const colors = {dependency: 'var(--accent)', after: 'var(--muted)', parent: 'var(--parent)', conflict: 'var(--bad)', related: 'var(--muted)'};
const dashes = {after: '8 5', conflict: '8 3 2 3', related: '2 5'};
const names = {dependency: 'Dependency', after: 'Ordering', parent: 'Parent → child', conflict: 'Conflict', related: 'Related'};
function RelationEdge(props) {
  const {sourceX: x, sourceY: y, targetX: tx, targetY: ty, data} = props;
  const clearance = 22 + ['parent', 'dependency', 'after', 'conflict', 'related'].indexOf(data.kind) * 13;
  const hierarchy = data.kind === 'parent';
  const source = data.rectangles.find(r => r.id === props.source), destination = data.rectangles.find(r => r.id === props.target);
  let points, path, dot;
  if (hierarchy) {
    const {index, count} = data.port;
    const exit = {x: source.x + source.width * (index + 1) / (count + 1), y: source.y + source.height}, entry = {x: destination.x + destination.width * (destination.parent ? 0.75 : 0.5), y: destination.y};
    const bend = Math.max(40, (entry.y - exit.y) / 2);
    const curve = t => {
      const u = 1 - t;
      return {x: (u ** 3 + 3 * u * u * t) * exit.x + (3 * u * t * t + t ** 3) * entry.x, y: u ** 3 * exit.y + 3 * u * u * t * (exit.y + bend) + 3 * u * t * t * (entry.y - bend) + t ** 3 * entry.y};
    };
    const samples = Array.from({length: 33}, (_, i) => curve(i / 32));
    const others = data.rectangles.filter(r => r.id !== props.source && r.id !== props.target);
    const direct = entry.y > exit.y + 20 && samples.every(p => others.every(r => p.x < r.x - 8 || p.x > r.x + r.width + 8 || p.y < r.y - 8 || p.y > r.y + r.height + 8));
    dot = exit;
    if (direct) {
      points = samples;
      path = `M ${exit.x},${exit.y} C ${exit.x},${exit.y + bend} ${entry.x},${entry.y - bend} ${entry.x},${entry.y}`;
    } else {
      const lane = 10 + 18 * index / Math.max(1, count - 1);
      const routed = connection(exit, entry, data.rectangles, lane, [{x: exit.x, y: exit.y + lane}, {x: entry.x, y: entry.y - lane}]);
      points = routed.length ? routed : [exit, entry];
    }
  } else {
    const start = {x: Math.max(x, source.x + source.width) + clearance, y};
    const end = {x: Math.min(tx, destination.x) - clearance, y: ty};
    const offset = {dependency: -28, after: 28, conflict: -56, related: 56}[data.kind];
    const waypoint = p => {
      let result = {x: p.x, y: p.y + offset};
      let obstacle;
      while ((obstacle = data.rectangles.find(r => result.x > r.x - clearance && result.x < r.x + r.width + clearance && result.y > r.y - clearance && result.y < r.y + r.height + clearance))) result.y = offset < 0 ? obstacle.y - clearance : obstacle.y + obstacle.height + clearance;
      return result;
    };
    const routed = connection({x: Math.max(x, source.x + source.width), y}, {x: Math.min(tx, destination.x), y: ty}, data.rectangles, clearance, [start, waypoint(start), waypoint(end), end]);
    points = routed.length ? [{x, y}, ...routed, {x: tx, y: ty}] : [];
  }
  path ||= points.map((p, i) => `${i ? 'L' : 'M'} ${p.x},${p.y}`).join(' ');
  const segments = points.slice(1).map((b, i) => ({a: points[i], b})).sort((a, b) => Math.hypot(b.a.x - b.b.x, b.a.y - b.b.y) - Math.hypot(a.a.x - a.b.x, a.a.y - a.b.y));
  const width = props.label.length * 6, height = 14;
  const candidates = segments.flatMap(({a, b}) => [.5, .25, .75].map(fraction => ({x: a.x + (b.x - a.x) * fraction + (a.x === b.x ? width / 2 + 8 : 0), y: a.y + (b.y - a.y) * fraction - 8})));
  const fits = p => [...data.rectangles, ...[...data.labels].filter(([id]) => id !== props.id).map(([, box]) => box)].every(r => p.x + width / 2 + 4 < r.x || p.x - width / 2 - 4 > r.x + r.width || p.y + 4 < r.y || p.y - height - 4 > r.y + r.height);
  const label = candidates.find(fits) || {x: (x + tx) / 2, y: Math.min(...data.rectangles.map(r => r.y)) - clearance};
  while (!fits(label)) label.y -= height + 8;
  data.labels.set(props.id, {x: label.x - width / 2, y: label.y - height, width, height});
  return <><BaseEdge id={props.id} path={path} style={props.style} markerEnd={props.markerEnd} />
    {dot && <circle className="hierarchy-dot" cx={dot.x} cy={dot.y} r={5} />}
    <text className="react-flow__edge-text" x={label.x} y={label.y} textAnchor="middle" style={props.labelStyle}>{props.label}</text></>;

}
const edgeTypes = {relation: RelationEdge};
export default function Graph({tasks, allTasks = tasks, select, workspace, activityEnabled = true}) {
  const key = `coordinator.graph.v1:${JSON.stringify(workspace)}`;
  const view = useRef(readView(key));
  const [positions, setPositions] = useState(view.current.positions);
  const [measurements, setMeasurements] = useState({});
  const [showDone, setShowDone] = useState(false);
  const [collapsed, setCollapsed] = useState(new Set());
  const [query, setQuery] = useState('');
  const [inspected, setInspected] = useState(null);
  const flow = useRef(null);
  const simulation = useRef(null);
  const frame = useRef(null);
  const tickLayout = useRef(null);
  const [running, setRunning] = useState(false);
  const [enabled, setEnabled] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(() => matchMedia('(prefers-reduced-motion: reduce)').matches);
  const byId = new Map(allTasks.map(task => [task.id, task]));
  const children = new Map();
  allTasks.forEach(task => {
    if (task.parent) children.set(task.parent, [...(children.get(task.parent) || []), task.id]);
  });
  const scope = new Set(tasks.map(task => task.id));
  const eligible = new Set(tasks.filter(task => (showDone || task.stage !== 'done' || tasks.some(other => other.stage !== 'done' && (other.dependencies.some(d => target(d) === task.id) || other.after.includes(task.id)))) && task.title.toLowerCase().includes(query.toLowerCase())).map(task => task.id));
  for (const id of [...eligible]) {
    const visited = new Set([id]);
    let parent = byId.get(id)?.parent;
    while (scope.has(parent) && !visited.has(parent)) {
      visited.add(parent); eligible.add(parent); parent = byId.get(parent)?.parent;
    }
  }
  const shown = tasks.filter(task => {
    if (!eligible.has(task.id)) return false;
    const visited = new Set([task.id]);
    let parent = task.parent;
    while (parent && !visited.has(parent)) {
      if (scope.has(parent) && collapsed.has(parent)) return false;
      visited.add(parent); parent = byId.get(parent)?.parent;
    }
    return true;
  });
  const pane = useRef(null);
  const framed = useRef(false);
  const [ready, setReady] = useState(false);
  const room = pane.current?.clientHeight ? {width: (pane.current.clientWidth - 2 * margin) / legible, height: (pane.current.clientHeight - 2 * margin) / legible, scale: legible} : undefined;
  const shownKey = JSON.stringify(shown.map(t => [t.id, t.title, t.parent, t.container, t.dependencies, t.after, t.conflicts, t.related]));
  const defaults = useMemo(() => arrange(shown, measurements, room), [shownKey, measurements, room?.width, room?.height]);
  const measured = shown.every(t => measurements[t.id]);
  const established = shown.some(t => point(positions[t.id])) ? extend(shown, positions, measurements) : {...positions, ...defaults};
  const placed = tasks.filter(t => point(established[t.id]));
  const missing = shown.some(t => !point(positions[t.id]));
  useEffect(() => {
    if (measured && missing) {setPositions(established); persist({positions: established});}
  }, [measured, missing, shownKey, positions]);
  const persist = patch => {
    view.current = {...view.current, ...patch}; views.set(key, view.current);
    try { localStorage.setItem(key, JSON.stringify(view.current)); } catch {}
  };
  const bounds = list => {
    const boxes = list.map(t => ({...established[t.id], width: measurements[t.id]?.width || 220, height: measurements[t.id]?.height || 80}));
    const x = Math.min(...boxes.map(b => b.x)), y = Math.min(...boxes.map(b => b.y));
    return {x, y, width: Math.max(...boxes.map(b => b.x + b.width)) - x, height: Math.max(...boxes.map(b => b.y + b.height)) - y};
  };
  const frameGraph = () => {
    const box = pane.current;
    if (!flow.current || !box?.clientWidth || !shown.length) return;
    const room = {width: box.clientWidth - 2 * margin, height: box.clientHeight - 2 * margin};
    const fit = area => Math.min(1, room.width / area.width, room.height / area.height);
    const all = bounds(shown), live = shown.filter(t => t.active && !t.stopped);
    const focus = fit(all) >= legible || !live.length ? all : bounds(live);
    const zoom = Math.max(fit(all), legible);
    const centered = fit(focus) >= zoom;
    const x = centered ? box.clientWidth / 2 - (focus.x + focus.width / 2) * zoom : margin - focus.x * zoom;
    const y = centered ? box.clientHeight / 2 - (focus.y + focus.height / 2) * zoom : margin - focus.y * zoom;
    flow.current.setViewport({x, y, zoom}, {duration: 0});
  };
  useEffect(() => {
    if (framed.current || !ready || !measured || missing) return;
    framed.current = true;
    const saved = view.current.viewport, box = pane.current;
    const seen = saved && box && shown.some(t => {
      const p = established[t.id], width = (measurements[t.id]?.width || 220) * saved.zoom, height = (measurements[t.id]?.height || 80) * saved.zoom;
      const x = p.x * saved.zoom + saved.x, y = p.y * saved.zoom + saved.y;
      return x + width > 0 && y + height > 0 && x < box.clientWidth && y < box.clientHeight;
    });
    if (!seen) frameGraph();
  });
  const stopLayout = () => {
    simulation.current?.cancel();
    simulation.current = null;
    cancelAnimationFrame(frame.current);
    frame.current = null;
    setRunning(false);
  };
  const startLayout = () => {
    stopLayout();
    const current = createSimulation(placed, established, measurements);
    simulation.current = current;
    setRunning(true);
    const tick = () => {
      if (simulation.current !== current) return;
      const result = current.step();
      setPositions(previous => ({...previous, ...result.positions}));
      persist({positions: {...view.current.positions, ...result.positions}});
      if (result.done) {frame.current = null; setRunning(false);}
      else frame.current = requestAnimationFrame(tick);
    };
    tickLayout.current = tick;
    frame.current = requestAnimationFrame(tick);
  };
  const wakeLayout = () => {
    if (simulation.current && frame.current === null) {setRunning(true); frame.current = requestAnimationFrame(tickLayout.current);}
  };
  const releaseNode = id => {simulation.current?.unpin(id); wakeLayout();};
  const layoutInputs = JSON.stringify(tasks.map(t => [t.id, t.parent, t.dependencies, t.after, t.conflicts, t.related]));
  useEffect(() => {
    if (enabled && !reducedMotion) startLayout();
    else stopLayout();
  }, [enabled, reducedMotion]);
  useEffect(() => {simulation.current?.update(placed, established, measurements); wakeLayout();}, [layoutInputs, measurements, placed.length]);
  useEffect(() => {
    const preference = matchMedia('(prefers-reduced-motion: reduce)');
    const changed = () => {setReducedMotion(preference.matches); if (preference.matches) {stopLayout(); setEnabled(false);}};
    preference.addEventListener('change', changed);
    return () => preference.removeEventListener('change', changed);
  }, []);
  useEffect(() => () => {simulation.current?.cancel(); cancelAnimationFrame(frame.current);}, []);
  const ids = new Set(shown.map(task => task.id));
  const nodes = shown.map(task => {
    const members = children.get(task.id) || [], parent = task.container || members.length > 0;
    const hidden = members.filter(id => !ids.has(id)).length;
    return {
      id: task.id, type: 'task', measured: measurements[task.id], position: established[task.id],
      className: `graph-task ${parent ? 'graph-parent' : ''} ${task.parent ? 'graph-subtask' : ''} ${task.blocked ? 'bad' : task.active && !task.stopped && activityEnabled ? 'live' : task.stage === 'done' ? 'good' : ''}`,
      ariaLabel: `${parent ? 'Parent: ' : ''}${task.title}. ${task.status}. Press Enter or Space to inspect.`,
      data: {active: !!task.active && !task.blocked && !task.stopped && activityEnabled, label: <>
        {parent && <span className="parent-label">Parent · {members.length} {members.length === 1 ? 'subtask' : 'subtasks'}</span>}
        {task.parent && <span className="subtask-label">Subtask</span>}
        <strong>{task.title}</strong><span>{task.status} · {task.attributes.lane}</span>
        {parent && <><span>{members.length} direct children · {hidden} hidden</span>
          <button className="nodrag nopan" aria-label={`${collapsed.has(task.id) ? 'Expand' : 'Collapse'} ${task.title}`} aria-expanded={!collapsed.has(task.id)} onClick={event => {
            event.stopPropagation();
            setCollapsed(previous => {const next = new Set(previous); if (next.has(task.id)) next.delete(task.id); else next.add(task.id); return next;});
          }}>{collapsed.has(task.id) ? '+ Expand' : '− Collapse'}</button></>}
        {task.parent && !ids.has(task.parent) && <span>Parent outside view: {byId.get(task.parent)?.title || task.parent}</span>}
      </>},
    };
  });
  const ports = new Map();
  shown.forEach(task => {
    const kids = (children.get(task.id) || []).filter(id => ids.has(id) && point(established[id]) && point(established[task.id]));
    const center = id => established[id].x + (measurements[id]?.width || 220) / 2;
    kids.sort((a, b) => center(a) - center(b) || established[a].y - established[b].y).forEach((id, index) => ports.set(id, {index, count: kids.length}));
  });
  const edges = [], relationships = [], seen = new Set(), labels = new Map();
  const endpoint = id => `${byId.get(id)?.title || 'Unavailable task'} [${id}]${ids.has(id) ? '' : ' (outside view)'}`;
  function edge(from, to, kind, reason) {
    if (!ids.has(from) && !ids.has(to)) return;
    const undirected = ['conflict', 'related'].includes(kind);
    if (undirected && from > to) [from, to] = [to, from];
    const id = JSON.stringify([kind, from, to]);
    if (seen.has(id)) return;
    seen.add(id);
    const label = kind === 'dependency' ? `dependency · ${reason ? reason.replaceAll('_', ' ') : 'unresolved reason'}` : names[kind].toLowerCase();
    const description = `${endpoint(from)} ${undirected ? '↔' : '→'} ${endpoint(to)} · ${label}`;
    relationships.push({id, from, to, kind, label, description});
    if (!ids.has(from) || !ids.has(to)) return;
    edges.push({id, source: from, target: to, label, ariaLabel: description, className: `graph-edge-${kind}`, data: {kind, labels, port: ports.get(to) || {index: 0, count: 1}, rectangles: nodes.map(n => ({id: n.id, parent: n.className.includes('graph-parent'), ...n.position, width: measurements[n.id]?.width || 220, height: measurements[n.id]?.height || 160}))},
      sourceHandle: kind === 'parent' ? 'hierarchy-out' : 'relations-out', targetHandle: kind === 'parent' ? 'hierarchy-in' : 'relations-in',
      type: 'relation',
      style: {stroke: colors[kind], strokeDasharray: dashes[kind], strokeWidth: kind === 'parent' ? 3 : kind === 'related' ? 1 : 1.5},
      markerEnd: ['dependency', 'after'].includes(kind) ? {type: MarkerType.ArrowClosed, color: colors[kind]} : undefined,
      labelStyle: {fill: 'var(--fg)'}, labelShowBg: false, reconnectable: false});
  }
  allTasks.forEach(task => {
    task.dependencies.forEach(d => edge(target(d), task.id, 'dependency', d.reason));
    task.after.forEach(id => edge(id, task.id, 'after'));
    if (task.parent) edge(task.parent, task.id, 'parent');
    task.conflicts.forEach(id => edge(id, task.id, 'conflict'));
    task.related.forEach(id => edge(id, task.id, 'related'));
  });
  const selectedRelation = relationships.find(edge => edge.id === inspected);
  return <>
    <div className="filters">
      <input type="search" aria-label="Filter graph by title" placeholder="Filter graph by title" value={query} onChange={event => setQuery(event.target.value)} />
      <label className="check"><input type="checkbox" checked={showDone} onChange={event => setShowDone(event.target.checked)} /> Show finished tasks</label>
      <button onClick={() => flow.current?.fitView({duration: 0})}>Fit graph</button>
      <button onClick={frameGraph}>Reset view</button>
      <button onClick={() => {stopLayout(); setEnabled(false); framed.current = false; persist({positions: {}, viewport: undefined}); setPositions({});}}>Arrange</button>
      <label className="check"><input type="checkbox" role="switch" aria-label="Graph physics" aria-describedby="physics-status" checked={enabled} disabled={reducedMotion} onChange={event => {if (!event.target.checked) stopLayout(); setEnabled(event.target.checked);}} /> Physics</label>
      <span className="muted" id="physics-status" role="status">{reducedMotion ? 'Physics paused for reduced motion. Manual dragging is available.' : enabled ? running ? 'Arranging tasks…' : 'Layout settled' : 'Physics off · positions frozen'}</span>
      <span className="legend" aria-label="Relationship legend">{Object.entries(names).map(([kind, name]) => <span key={kind}><i className={kind} />{name}{['dependency', 'after'].includes(kind) ? ' →' : ''}</span>)}</span>
    </div>
    <p className="muted" id="graph-help">Drag tasks to arrange. Drag the background to pan. Scroll or pinch to zoom. Tab to a task and press Enter or Space to inspect; arrow keys move it. Hidden counts refer to immediate children. Focus or select an edge to inspect its full relationship. Supersession remains in task history.</p>
    {!shown.length && <p className="empty">No matching tasks in this scope.</p>}
    <div className="graph" ref={pane} onKeyUpCapture={event => {if (event.key.startsWith('Arrow')) releaseNode(event.target.closest('.react-flow__node')?.dataset.id);}} onFocusCapture={event => {const edge = event.target.closest(".react-flow__edge"); if (edge) setInspected(edge.dataset.id);}} onKeyDownCapture={event => {
      if (event.target.closest('button, input')) return;
      const node = event.target.closest('.react-flow__node');
      const direction = {ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1]}[event.key];
      if (node && direction) {
        event.preventDefault(); event.stopPropagation();
        setPositions(previous => {
          const current = previous[node.dataset.id] || defaults[node.dataset.id], step = event.shiftKey ? 50 : 10;
          const next = {...previous, [node.dataset.id]: {x: current.x + direction[0] * step, y: current.y + direction[1] * step}};
          simulation.current?.pin(node.dataset.id, next[node.dataset.id]);
          persist({positions: next}); return next;
        });
      }
      if (node && (event.key === 'Enter' || event.key === ' ')) {
        event.preventDefault(); event.stopPropagation(); select(node.dataset.id);
      }
    }}>
      <ReactFlow onInit={instance => {flow.current = instance; setReady(true);}} minZoom={0.01} edgeTypes={edgeTypes} edgesFocusable onEdgeClick={(_, edge) => setInspected(edge.id)} onSelectionChange={({edges}) => {if (edges.length) setInspected(edges[0].id);}} nodeTypes={nodeTypes} nodes={nodes} edges={edges} aria-label="Task dependency graph" aria-describedby="graph-help"
        defaultViewport={view.current.viewport} nodesConnectable={false} edgesReconnectable={false}
        deleteKeyCode={null} nodeDragThreshold={5} nodeClickDistance={5} panOnDrag zoomOnScroll zoomOnPinch
        onNodeClick={(_, node) => select(node.id)}
        onNodeDragStart={(_, node) => simulation.current?.pin(node.id, node.position)}
        onNodeDragStop={(_, node) => releaseNode(node.id)}
        onNodesChange={changes => {
          const measured = changes.filter(change => change.type === 'dimensions' && change.dimensions);
          if (measured.length) setMeasurements(previous => ({...previous, ...Object.fromEntries(measured.map(change => [change.id, change.dimensions]))}));
          const moved = changes.filter(change => change.type === 'position' && point(change.position));
          if (!moved.length) return;
          moved.forEach(change => simulation.current?.pin(change.id, change.position));
          setPositions(previous => {
            const next = {...previous, ...Object.fromEntries(moved.map(change => [change.id, change.position]))};
            persist({positions: next}); return next;
          });
        }} onMoveEnd={(_, viewport) => persist({viewport})}>
        <Background color="var(--line)" /><Controls showInteractive={false} />
        <MiniMap pannable zoomable nodeColor="var(--accent)" maskColor="var(--bg)" />
      </ReactFlow>
    </div>
    <p className="graph-inspection" aria-live="polite">{selectedRelation?.description || 'Select an edge or expand relationship inspection for full endpoints and labels.'}</p>
    <details className="graph-relationships"><summary>Inspect relationships ({relationships.length})</summary>
      <ul>{relationships.map(edge => <li key={edge.id}>
        <strong>{names[edge.kind]}</strong>: <button className="link" disabled={!byId.has(edge.from)} onClick={() => select(edge.from)}>{endpoint(edge.from)}</button>
        {['conflict', 'related'].includes(edge.kind) ? ' ↔ ' : ' → '}
        <button className="link" disabled={!byId.has(edge.to)} onClick={() => select(edge.to)}>{endpoint(edge.to)}</button>
        {edge.kind === 'dependency' && <span> · Reason: {edge.label.slice('dependency · '.length)}</span>}
      </li>)}</ul>
    </details>
  </>;
}
