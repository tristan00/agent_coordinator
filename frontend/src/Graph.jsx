import React, {useEffect, useMemo, useRef, useState} from 'react';
import {ReactFlow, Background, Controls, MiniMap, MarkerType, Position, Handle, BaseEdge, Panel} from '@xyflow/react';
import {ArrowDown, ArrowRight, Atom, Crosshair, Eye, HelpCircle, LayoutGrid, Maximize2} from 'lucide-react';
import {FieldPicker, usePreference} from './Controls';
import {FIELD, display, colorScale} from './fields';
import '@xyflow/react/dist/style.css';
import {relationLayout, hierarchyGeometry, placeLabels, directGeometry} from './edges';
import {createSimulation, arrange, extend} from './layout';
import {BorderBeam} from './Activity';
import TaskName, {taskLabel} from './TaskName';

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
function RelationEdge({id, style, markerEnd, labelStyle, data}) {
  const {kind, path, start, end, text} = data;
  const undirected = ['conflict', 'related'].includes(kind);
  return <><BaseEdge id={id} path={path} style={style} markerEnd={markerEnd} className={data.fresh ? 'edge-enter' : ''} />
    {data.fresh && <path d={path} pathLength="1" className="edge-sweep" style={{stroke: colors[kind]}} fill="none" />}
    {data.flow && <path d={path} className="edge-flow" style={{stroke: 'var(--live)'}} fill="none" />}
    <circle className={kind === 'parent' ? 'hierarchy-dot' : 'edge-dot'} cx={start.x} cy={start.y} r={kind === 'parent' ? 5 : 3.5} style={{fill: colors[kind]}} />
    {undirected && <circle className="edge-dot" cx={end.x} cy={end.y} r={3.5} style={{fill: colors[kind]}} />}
    <title>{data.tooltip}</title>{text && <text className="react-flow__edge-text graph-edge-reason" x={text.x} y={text.y} textAnchor="middle" dominantBaseline="central" transform={`rotate(${text.angle} ${text.x} ${text.y})`} style={labelStyle}>{text.label}</text>}</>;
}
const edgeTypes = {relation: RelationEdge};
export default function Graph({tasks, allTasks = tasks, select, workspace, activityEnabled = true}) {
  const [direction, setDirection] = usePreference('coordinator.graph.direction', 'down');
  const key = `coordinator.graph.v2:${direction}:${JSON.stringify(workspace)}`;
  const view = useRef(readView(key));
  const [positions, setPositions] = useState(view.current.positions);
  const [measurements, setMeasurements] = useState({});
  const [showDone, setShowDone] = usePreference('coordinator.graph.done', false);
  const [fields, setFields] = usePreference('coordinator.graph.fields', ['status', 'role']);
  const [colorBy, setColorBy] = usePreference('coordinator.graph.color', '');
  const [collapsed, setCollapsed] = useState(new Set());
  const seen = useRef(null);
  const edgesSeen = useRef(null);
  const [inspected, setInspected] = useState(null);
  const flow = useRef(null);
  const simulation = useRef(null);
  const frame = useRef(null);
  const tickLayout = useRef(null);
  const [running, setRunning] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [enabled, setEnabled] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(() => matchMedia('(prefers-reduced-motion: reduce)').matches);
  const byId = new Map(allTasks.map(task => [task.id, task]));
  const children = new Map();
  allTasks.forEach(task => {
    if (task.parent) children.set(task.parent, [...(children.get(task.parent) || []), task.id]);
  });
  const scope = new Set(tasks.map(task => task.id));
  const eligible = new Set(tasks.filter(task => (showDone || task.stage !== 'done' || tasks.some(other => other.stage !== 'done' && (other.dependencies.some(d => target(d) === task.id) || other.after.includes(task.id))))).map(task => task.id));
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
  const drawn = useRef([]);
  const slots = useRef(new Map());
  const routeCache = useRef(new Map());
  const labelCache = useRef(new Map());
  const [ready, setReady] = useState(false);
  const room = pane.current?.clientHeight ? {width: (pane.current.clientWidth - 2 * margin) / legible, height: (pane.current.clientHeight - 2 * margin) / legible, scale: legible} : undefined;
  const shownKey = JSON.stringify(shown.map(t => [t.id, t.title, t.parent, t.container, t.dependencies, t.after, t.conflicts, t.related]));
  const defaults = useMemo(() => arrange(shown, measurements, room, direction), [shownKey, measurements, room?.width, room?.height, direction]);
  const measured = shown.every(t => measurements[t.id]);
  const established = shown.some(t => point(positions[t.id])) ? extend(shown, positions, measurements, direction) : {...positions, ...defaults};
  const placed = tasks.filter(t => point(established[t.id]));
  const missing = shown.some(t => !point(positions[t.id]));
  useEffect(() => {
    if (measured && missing) {setPositions(established); persist({positions: established});}
  }, [measured, missing, shownKey, positions]);
  const persist = patch => {
    view.current = {...view.current, ...patch}; views.set(key, view.current);
    try { localStorage.setItem(key, JSON.stringify(view.current)); } catch {}
  };
  const bounds = (list, extra = []) => {
    const boxes = [...list.map(t => ({...established[t.id], width: measurements[t.id]?.width || 220, height: measurements[t.id]?.height || 80})), ...extra];
    const x = Math.min(...boxes.map(b => b.x)), y = Math.min(...boxes.map(b => b.y));
    return {x, y, width: Math.max(...boxes.map(b => b.x + b.width)) - x, height: Math.max(...boxes.map(b => b.y + b.height)) - y};
  };
  const frameGraph = () => {
    const box = pane.current;
    if (!flow.current || !box?.clientWidth || !shown.length) return;
    const room = {width: box.clientWidth - 2 * margin, height: box.clientHeight - 2 * margin};
    const fit = area => Math.min(1, room.width / area.width, room.height / area.height);
    const all = bounds(shown, drawn.current), live = shown.filter(t => t.active && !t.stopped);
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
    persist({});
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
      view.current = {...view.current, positions: {...view.current.positions, ...result.positions}};
      if (result.done) {frame.current = null; setRunning(false); persist({});}
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
  const now = Date.now();
  if (!seen.current) seen.current = new Map(shown.map(task => [task.id, {at: 0, status: task.status, changed: 0}]));
  shown.forEach(task => {
    const memory = seen.current.get(task.id);
    if (!memory) seen.current.set(task.id, {at: now, status: task.status, changed: 0});
    else if (memory.status !== task.status) seen.current.set(task.id, {...memory, status: task.status, changed: now});
  });
  const scale = colorScale(FIELD[colorBy] || FIELD.status, shown);
  const nodes = shown.map(task => {
    const members = children.get(task.id) || [], parent = task.container || members.length > 0;
    const hidden = members.filter(id => !ids.has(id)).length;
    return {
      id: task.id, type: 'task', measured: measurements[task.id], position: established[task.id],
      className: `graph-task ${parent ? 'graph-parent' : ''} ${task.parent ? 'graph-subtask' : ''} ${task.blocked ? 'bad' : task.active && !task.stopped && activityEnabled ? 'live' : task.stage === 'done' ? 'good' : ''} ${task.stopped ? 'graph-stopped' : ''} ${now - seen.current.get(task.id).at < 900 ? 'graph-enter' : ''} ${now - seen.current.get(task.id).changed < 1600 ? 'graph-changed' : ''}`,
      style: {'--node-accent': scale.color(task)},
      ariaLabel: `${parent ? 'Parent: ' : ''}${taskLabel(task)}. ${task.status}. Press Enter or Space to inspect.`,
      data: {active: !!task.active && !task.blocked && !task.stopped && activityEnabled, label: <>
        {parent && <span className="parent-label">Parent · {members.length} {members.length === 1 ? 'subtask' : 'subtasks'}</span>}
        {task.parent && <span className="subtask-label">Subtask</span>}
        <strong><TaskName task={task} /></strong>
        {!!fields.length && <span className="node-fields">{fields.filter(key => FIELD[key]).map(key => <span className="node-field" key={key}><i>{FIELD[key].label}</i><b>{display(FIELD[key], task)}</b></span>)}</span>}
        {parent && <><span>{members.length} direct children · {hidden} hidden</span>
          <button className="nodrag nopan" aria-label={`${collapsed.has(task.id) ? 'Expand' : 'Collapse'} ${task.title}`} aria-expanded={!collapsed.has(task.id)} onClick={event => {
            event.stopPropagation();
            setCollapsed(previous => {const next = new Set(previous); if (next.has(task.id)) next.delete(task.id); else next.add(task.id); return next;});
          }}>{collapsed.has(task.id) ? '+ Expand' : '− Collapse'}</button></>}
        {task.parent && !ids.has(task.parent) && <span>Parent outside view: {byId.get(task.parent)?.title || task.parent}</span>}
      </>},
    };
  });
  const rectangles = nodes.map(n => ({id: n.id, parent: n.className.includes('graph-parent'), ...n.position, width: measurements[n.id]?.width || 220, height: measurements[n.id]?.height || 80}));
  const endpoint = id => `${byId.get(id) ? taskLabel(byId.get(id)) : 'Unavailable task'} [${id}]${ids.has(id) ? '' : ' (outside view)'}`;
  const moving = running || dragging;
  const geometryKey = JSON.stringify([rectangles, allTasks.map(t => [t.id, t.title, t.parent, t.dependencies, t.after, t.conflicts, t.related])]);
  const {edges, relationships} = useMemo(() => {
  const ports = new Map();
  shown.forEach(task => {
    const kids = (children.get(task.id) || []).filter(id => ids.has(id) && point(established[id]) && point(established[task.id]));
    const center = id => established[id].x + (measurements[id]?.width || 220) / 2;
    kids.sort((a, b) => center(a) - center(b) || established[a].y - established[b].y).forEach((id, index) => ports.set(id, {index, count: kids.length}));
  });
  const edges = [], relationships = [], seen = new Set();
  const rectangle = new Map(rectangles.map(r => [r.id, r]));
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
    const source = rectangle.get(from), destination = rectangle.get(to);
    if (!point(source) || !point(destination)) return;
    const geometry = moving ? {...directGeometry(kind, source, destination, direction), quick: true} : kind === 'parent' ? hierarchyGeometry(source, destination, ports.get(to) || {index: 0, count: 1}, rectangles) : {source, destination};
    if (edgesSeen.current && !edgesSeen.current.has(id)) edgesSeen.current.set(id, now);
    const fresh = edgesSeen.current?.get(id) || 0;
    edges.push({id, source: from, target: to, label: kind === 'parent' ? undefined : kind === 'dependency' ? label.slice('dependency · '.length) : label, ariaLabel: description, className: `graph-edge-${kind}`, data: {kind, ...geometry, fresh: now - fresh < 1200, flow: activityEnabled && ['dependency', 'after'].includes(kind) && !!byId.get(to)?.active && !byId.get(to)?.stopped},
      sourceHandle: kind === 'parent' ? 'hierarchy-out' : 'relations-out', targetHandle: kind === 'parent' ? 'hierarchy-in' : 'relations-in',
      type: 'relation',
      style: {stroke: colors[kind], strokeDasharray: dashes[kind], strokeWidth: kind === 'parent' ? 3 : kind === 'related' ? 1 : 1.5},
      markerEnd: kind === 'parent' ? {type: MarkerType.ArrowClosed, color: colors[kind], width: 6, height: 6} : ['dependency', 'after'].includes(kind) ? {type: MarkerType.ArrowClosed, color: colors[kind], width: 14, height: 14} : undefined,
      labelStyle: {fill: 'var(--fg)'}, labelShowBg: false, reconnectable: false});
  }
  allTasks.forEach(task => {
    task.dependencies.forEach(d => edge(target(d), task.id, 'dependency', d.reason));
    task.after.forEach(id => edge(id, task.id, 'after'));
    if (task.parent) edge(task.parent, task.id, 'parent');
    task.conflicts.forEach(id => edge(id, task.id, 'conflict'));
    task.related.forEach(id => edge(id, task.id, 'related'));
  });
  const routes = moving ? new Map() : relationLayout(edges.filter(e => e.data.kind !== 'parent').map(e => ({id: e.id, kind: e.data.kind, source: e.data.source, destination: e.data.destination})), rectangles, direction);
  edges.forEach(e => {if (routes.has(e.id)) e.data = {kind: e.data.kind, fresh: e.data.fresh, flow: e.data.flow, ...routes.get(e.id)};});
  const box = r => JSON.stringify([r.x, r.y, r.width, r.height]);
  const blockedBy = (points, source, destination) => (points || []).slice(2, -2).filter(p => rectangles.some(r => r.id !== source && r.id !== destination && p.x > r.x - 4 && p.x < r.x + r.width + 4 && p.y > r.y - 4 && p.y < r.y + r.height + 4)).length;
  const stable = new Set(), nextRoutes = new Map(), degree = new Map();
  edges.forEach(e => [e.source, e.target].forEach(id => degree.set(id, (degree.get(id) || 0) + 1)));
  edges.forEach(e => {
    const sig = box(rectangle.get(e.source)) + box(rectangle.get(e.target)) + degree.get(e.source) + ':' + degree.get(e.target);
    const cached = routeCache.current.get(e.id);
    if (cached && cached.sig === sig && (moving || (!cached.data.quick && blockedBy(cached.data.points, e.source, e.target) <= cached.blocked))) {e.data = {...cached.data, fresh: e.data.fresh, flow: e.data.flow}; stable.add(e.id);}
    nextRoutes.set(e.id, stable.has(e.id) ? cached : {sig, data: e.data, blocked: blockedBy(e.data.points, e.source, e.target)});
  });
  routeCache.current = nextRoutes;
  const hits = (b, r) => b.x0 < r.x + r.width + 2 && r.x - 2 < b.x1 && b.y0 < r.y + r.height + 2 && r.y - 2 < b.y1;
  const edgeById = new Map(edges.map(e => [e.id, e]));
  const kept = new Map([...labelCache.current].filter(([id, t]) => stable.has(id) && edgeById.get(id)?.data.path === t.path && (edgeById.get(id)?.label || '').endsWith(t.label) && (moving || !rectangles.some(r => hits(t.bounds, r)))));
  const memory = new Map([...slots.current].filter(([id]) => kept.has(id)));
  const reserved = [...kept.values()].map(({bounds: b}) => ({x: b.x0, y: b.y0, width: b.x1 - b.x0, height: b.y1 - b.y0}));
  const reasons = moving ? kept : new Map([...placeLabels(edges.map(e => ({id: e.id, text: kept.has(e.id) ? '' : e.label || '', points: e.data.points})), [...rectangles, ...reserved], slots.current), ...kept]);
  if (!moving) memory.forEach((slot, id) => slots.current.set(id, slot));
  if (!moving) labelCache.current = new Map([...reasons].map(([id, t]) => [id, {...t, path: edgeById.get(id)?.data.path}]));
  edges.forEach(e => {e.data = {...e.data, text: reasons.get(e.id), tooltip: e.ariaLabel};});
  if (!edgesSeen.current) edgesSeen.current = new Map(edges.map(e => [e.id, 0]));
  if (!moving) drawn.current = [...edges.flatMap(e => e.data.points || []).map(p => ({x: p.x, y: p.y, width: 0, height: 0})), ...[...reasons.values()].map(({bounds: b}) => ({x: b.x0, y: b.y0, width: b.x1 - b.x0, height: b.y1 - b.y0}))];
  return {edges, relationships};
  }, [geometryKey, moving, showDone, collapsed, direction, JSON.stringify(allTasks.map(t => [t.id, !!t.active, t.stopped])), activityEnabled]);
  const selectedRelation = relationships.find(edge => edge.id === inspected);
  return <>
    <p className="sr-only" id="graph-help">Drag tasks to arrange. Drag the background to pan. Scroll or pinch to zoom. Tab to a task and press Enter or Space to inspect; arrow keys move it. Hidden counts refer to immediate children. Focus or select an edge to inspect its full relationship.</p>
    {!shown.length && <p className="empty">No matching tasks in this scope.</p>}
    <div className="graph" ref={pane} onKeyUpCapture={event => {if (event.key.startsWith('Arrow')) persist({}); if (event.key.startsWith('Arrow')) releaseNode(event.target.closest('.react-flow__node')?.dataset.id);}} onFocusCapture={event => {const edge = event.target.closest(".react-flow__edge"); if (edge) setInspected(edge.dataset.id);}} onKeyDownCapture={event => {
      if (event.target.closest('button, input')) return;
      const node = event.target.closest('.react-flow__node');
      const direction = {ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1]}[event.key];
      if (node && direction) {
        event.preventDefault(); event.stopPropagation();
        setPositions(previous => {
          const current = previous[node.dataset.id] || defaults[node.dataset.id], step = event.shiftKey ? 50 : 10;
          const next = {...previous, [node.dataset.id]: {x: current.x + direction[0] * step, y: current.y + direction[1] * step}};
          simulation.current?.pin(node.dataset.id, next[node.dataset.id]);
          view.current = {...view.current, positions: next}; return next;
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
        onNodeDragStart={(_, node) => {setDragging(true); simulation.current?.pin(node.id, node.position);}}
        onNodeDragStop={(_, node) => {setDragging(false); persist({}); releaseNode(node.id);}}
        onNodesChange={changes => {
          const measured = changes.filter(change => change.type === 'dimensions' && change.dimensions);
          if (measured.length) setMeasurements(previous => ({...previous, ...Object.fromEntries(measured.map(change => [change.id, change.dimensions]))}));
          const moved = changes.filter(change => change.type === 'position' && point(change.position));
          if (!moved.length) return;
          moved.forEach(change => simulation.current?.pin(change.id, change.position));
          setPositions(previous => {
            const next = {...previous, ...Object.fromEntries(moved.map(change => [change.id, change.position]))};
            view.current = {...view.current, positions: next}; return next;
          });
        }} onMoveEnd={(_, viewport) => persist({viewport})}>
        <Panel position="top-left" className="graph-toolbar" aria-label="Graph view settings">
          <FieldPicker fields={fields} setFields={setFields} colorBy={colorBy} setColorBy={setColorBy} />
          <span className="toolbar-sep" />
          <button type="button" className={`tool ${showDone ? 'on' : ''}`} aria-pressed={showDone} onClick={() => setShowDone(v => !v)} title="Show finished tasks"><Eye size={15} aria-hidden="true" />Finished</button>
          <button type="button" role="switch" aria-checked={enabled} aria-label="Graph physics" aria-describedby="physics-status" className={`tool ${enabled ? 'on' : ''}`} disabled={reducedMotion} onClick={() => {if (enabled) stopLayout(); setEnabled(!enabled);}} title={reducedMotion ? 'Physics paused for reduced motion' : 'Physics layout'}><Atom size={15} aria-hidden="true" className={enabled && running ? 'spin' : ''} />Physics</button>
          <button type="button" className="tool" onClick={() => {const next = direction === 'down' ? 'right' : 'down'; stopLayout(); setEnabled(false); framed.current = false; setDirection(next);}} title="Switch layout direction" aria-label={`Layout direction: ${direction === 'down' ? 'top to bottom' : 'left to right'}`}>{direction === 'down' ? <ArrowDown size={15} aria-hidden="true" /> : <ArrowRight size={15} aria-hidden="true" />}{direction === 'down' ? 'Top-down' : 'Left-right'}</button>
          <span className="toolbar-sep" />
          <button type="button" className="tool icon" onClick={() => flow.current?.fitView({duration: reducedMotion ? 0 : 300})} title="Fit graph" aria-label="Fit graph"><Maximize2 size={15} aria-hidden="true" /></button>
          <button type="button" className="tool icon" onClick={frameGraph} title="Reset view" aria-label="Reset view"><Crosshair size={15} aria-hidden="true" /></button>
          <button type="button" className="tool icon" onClick={() => {stopLayout(); setEnabled(false); framed.current = false; persist({positions: {}, viewport: undefined}); setPositions({});}} title="Arrange" aria-label="Arrange"><LayoutGrid size={15} aria-hidden="true" /></button>
          <span className="tool icon help" tabIndex={0} title="Drag tasks to arrange. Drag the background to pan. Scroll or pinch to zoom. Tab to a task and press Enter or Space to inspect; arrow keys move it. Focus or select an edge to inspect its full relationship." aria-label="Graph help"><HelpCircle size={15} aria-hidden="true" /></span>
          <span className="sr-only" id="physics-status" role="status">{reducedMotion ? 'Physics paused for reduced motion. Manual dragging is available.' : enabled ? running ? 'Arranging tasks…' : 'Layout settled' : 'Physics off · positions frozen'}</span>
        </Panel>
        <Panel position="bottom-left" className="graph-legend" aria-label="Legend">
          <div className="legend-block"><span className="legend-title">{(FIELD[colorBy] || FIELD.status).label}</span>
            {scale.gradient ? <span className="legend-gradient"><span>{scale.legend[0].label}</span><i style={{background: `linear-gradient(90deg, ${scale.legend[0].color}, hsl(115 80% 60%), ${scale.legend[1].color})`}} /><span>{scale.legend[1].label}</span></span>
              : <span className="legend-swatches">{scale.legend.slice(0, 10).map(item => <span key={item.label}><i style={{background: item.color}} />{item.label}</span>)}{scale.legend.length > 10 && <span className="muted">+{scale.legend.length - 10}</span>}</span>}
          </div>
          <div className="legend-block legend">{Object.entries(names).map(([kind, name]) => <span key={kind}><i className={kind} />{name}</span>)}</div>
        </Panel>
        <Background color="var(--line)" /><Controls showInteractive={false} position="bottom-right" />
        <MiniMap pannable zoomable nodeColor={node => node.style?.['--node-accent'] || 'var(--accent)'} maskColor="var(--bg)" position="top-right" />
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
