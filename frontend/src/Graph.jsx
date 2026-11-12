import React, {useEffect, useMemo, useRef, useState} from 'react';
import {ReactFlow, Background, Controls, MiniMap, MarkerType, Position, Handle, BaseEdge} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import {connection, arrange} from './routing';
import {createSimulation} from './layout';
import {BorderBeam} from './Activity';

const views = new Map();
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
const colors = {dependency: 'var(--accent)', after: 'var(--muted)', parent: 'var(--good)', conflict: 'var(--bad)', related: 'var(--muted)'};
const dashes = {after: '8 5', conflict: '8 3 2 3', related: '2 5'};
const names = {dependency: 'Dependency', after: 'Ordering', parent: 'Parent → child', conflict: 'Conflict', related: 'Related'};
function RelationEdge(props) {
  const {sourceX: x, sourceY: y, targetX: tx, targetY: ty, data} = props;
  const clearance = 22 + ['parent', 'dependency', 'after', 'conflict', 'related'].indexOf(data.kind) * 13;
  const hierarchy = data.kind === 'parent';
  const source = data.rectangles.find(r => r.id === props.source), destination = data.rectangles.find(r => r.id === props.target);
  const start = {x: hierarchy ? x : Math.max(x, source.x + source.width) + clearance, y: hierarchy ? Math.max(y, source.y + source.height) + clearance : y};
  const end = {x: hierarchy ? tx : Math.min(tx, destination.x) - clearance, y: hierarchy ? Math.min(ty, destination.y) - clearance : ty};
  const offset = {parent: 0, dependency: -28, after: 28, conflict: -56, related: 56}[data.kind];
  const waypoint = p => {
    let result = {x: p.x, y: p.y + offset};
    let obstacle;
    while ((obstacle = data.rectangles.find(r => result.x > r.x - clearance && result.x < r.x + r.width + clearance && result.y > r.y - clearance && result.y < r.y + r.height + clearance))) result.y = offset < 0 ? obstacle.y - clearance : obstacle.y + obstacle.height + clearance;
    return result;
  };
  const middle = offset ? [start, waypoint(start), waypoint(end), end] : [start, end];
  const exit = {x: hierarchy ? x : Math.max(x, source.x + source.width), y: hierarchy ? Math.max(y, source.y + source.height) : y};
  const entry = {x: hierarchy ? tx : Math.min(tx, destination.x), y: hierarchy ? Math.min(ty, destination.y) : ty};
  const routed = connection(exit, entry, data.rectangles, clearance, middle);
  const points = routed.length ? [{x, y}, ...routed, {x: tx, y: ty}] : [];
  const path = points.map((p, i) => `${i ? 'L' : 'M'} ${p.x},${p.y}`).join(' ');
  const segments = points.slice(1).map((b, i) => ({a: points[i], b})).sort((a, b) => Math.hypot(b.a.x - b.b.x, b.a.y - b.b.y) - Math.hypot(a.a.x - a.b.x, a.a.y - a.b.y));
  const width = props.label.length * 6, height = 14;
  const candidates = segments.flatMap(({a, b}) => [.5, .25, .75].map(fraction => ({x: a.x + (b.x - a.x) * fraction + (a.x === b.x ? width / 2 + 8 : 0), y: a.y + (b.y - a.y) * fraction - 8})));
  const fits = p => [...data.rectangles, ...[...data.labels].filter(([id]) => id !== props.id).map(([, box]) => box)].every(r => p.x + width / 2 + 4 < r.x || p.x - width / 2 - 4 > r.x + r.width || p.y + 4 < r.y || p.y - height - 4 > r.y + r.height);
  const label = candidates.find(fits) || {x: (x + tx) / 2, y: Math.min(...data.rectangles.map(r => r.y)) - clearance};
  while (!fits(label)) label.y -= height + 8;
  data.labels.set(props.id, {x: label.x - width / 2, y: label.y - height, width, height});
  return <><BaseEdge id={props.id} path={path} style={props.style} markerEnd={props.markerEnd} />
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
  const defaults = useMemo(() => arrange(tasks, measurements), [tasks, measurements]);
  const established = {...positions};
  for (const task of tasks) {
    if (established[task.id]) continue;
    let candidate = {...defaults[task.id]};
    const height = measurements[task.id]?.height || 160 + Math.ceil(task.title.length / 22) * 20;
    const collisions = () => Object.entries(established).filter(([id, p]) => Math.abs(p.x - candidate.x) < 300 && candidate.y < p.y + (measurements[id]?.height || 240) + 80 && candidate.y + height + 80 > p.y);
    while (collisions().length) candidate.y = Math.max(...collisions().map(([id, p]) => p.y + (measurements[id]?.height || 240) + 110));
    established[task.id] = candidate;
  }
  useEffect(() => {
    if (tasks.some(t => !positions[t.id])) {setPositions(established); persist({positions: established});}
  }, [tasks, positions]);
  const persist = patch => {
    view.current = {...view.current, ...patch}; views.set(key, view.current);
    try { localStorage.setItem(key, JSON.stringify(view.current)); } catch {}
  };
  const stopLayout = () => {
    simulation.current?.cancel();
    simulation.current = null;
    cancelAnimationFrame(frame.current);
    frame.current = null;
    setRunning(false);
  };
  const startLayout = () => {
    stopLayout();
    const current = createSimulation(tasks, established, measurements);
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
  useEffect(() => {simulation.current?.update(tasks, established, measurements); wakeLayout();}, [layoutInputs, measurements]);
  useEffect(() => {
    const preference = matchMedia('(prefers-reduced-motion: reduce)');
    const changed = () => {setReducedMotion(preference.matches); if (preference.matches) {stopLayout(); setEnabled(false);}};
    preference.addEventListener('change', changed);
    return () => preference.removeEventListener('change', changed);
  }, []);
  useEffect(() => () => {simulation.current?.cancel(); cancelAnimationFrame(frame.current);}, []);
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
  const ids = new Set(shown.map(task => task.id));
  const nodes = shown.map(task => {
    const members = children.get(task.id) || [], parent = task.container || members.length > 0;
    const hidden = members.filter(id => !ids.has(id)).length;
    return {
      id: task.id, type: 'task', measured: measurements[task.id], position: established[task.id],
      className: `graph-task ${parent ? 'graph-parent' : ''} ${task.blocked ? 'bad' : task.active && !task.stopped && activityEnabled ? 'live' : task.stage === 'done' ? 'good' : ''}`,
      ariaLabel: `${parent ? 'Parent: ' : ''}${task.title}. ${task.status}. Press Enter or Space to inspect.`,
      data: {active: !!task.active && !task.blocked && !task.stopped && activityEnabled, label: <>
        {parent && <span className="parent-label">Parent</span>}
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
    edges.push({id, source: from, target: to, label, ariaLabel: description, className: `graph-edge-${kind}`, data: {kind, labels, rectangles: nodes.map(n => ({id: n.id, ...n.position, width: measurements[n.id]?.width || 220, height: measurements[n.id]?.height || 160}))},
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
      <button onClick={() => flow.current?.setViewport({x: 0, y: 0, zoom: 1}, {duration: 0})}>Reset view</button>
      <label className="check"><input type="checkbox" role="switch" aria-label="Graph physics" aria-describedby="physics-status" checked={enabled} disabled={reducedMotion} onChange={event => {if (!event.target.checked) stopLayout(); setEnabled(event.target.checked);}} /> Physics</label>
      <span className="muted" id="physics-status" role="status">{reducedMotion ? 'Physics paused for reduced motion. Manual dragging is available.' : enabled ? running ? 'Arranging tasks…' : 'Layout settled' : 'Physics off · positions frozen'}</span>
      <span className="legend" aria-label="Relationship legend">{Object.entries(names).map(([kind, name]) => <span key={kind}><i className={kind} />{name}{['dependency', 'after'].includes(kind) ? ' →' : ''}</span>)}</span>
    </div>
    <p className="muted" id="graph-help">Drag tasks to arrange. Drag the background to pan. Scroll or pinch to zoom. Tab to a task and press Enter or Space to inspect; arrow keys move it. Hidden counts refer to immediate children. Focus or select an edge to inspect its full relationship. Supersession remains in task history.</p>
    {!shown.length && <p className="empty">No matching tasks in this scope.</p>}
    <div className="graph" onKeyUpCapture={event => {if (event.key.startsWith('Arrow')) releaseNode(event.target.closest('.react-flow__node')?.dataset.id);}} onFocusCapture={event => {const edge = event.target.closest(".react-flow__edge"); if (edge) setInspected(edge.dataset.id);}} onKeyDownCapture={event => {
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
      <ReactFlow onInit={instance => {flow.current = instance;}} minZoom={0.01} edgeTypes={edgeTypes} edgesFocusable onEdgeClick={(_, edge) => setInspected(edge.id)} onSelectionChange={({edges}) => {if (edges.length) setInspected(edges[0].id);}} nodeTypes={nodeTypes} nodes={nodes} edges={edges} aria-label="Task dependency graph" aria-describedby="graph-help"
        defaultViewport={view.current.viewport} fitView={!view.current.viewport} nodesConnectable={false} edgesReconnectable={false}
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
