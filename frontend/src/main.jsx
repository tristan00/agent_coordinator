import React, {useState, useEffect, useRef, useCallback, useMemo} from 'react';
import {createRoot} from 'react-dom/client';
import './style.css';

const STATUSES = ['To do', 'In progress', 'Review', 'Done'];
const LANES = ['code', 'ui', 'data', 'docs', 'audit', 'cleanup'];
const NEEDS = ['network', 'browser', 'process', 'gpu'];
const SIZES = ['small', 'medium', 'large'];
const ARTIFACTS = ['none', 'candidate', 'report'];
const EFFORTS = ['', 'minimal', 'low', 'medium', 'high', 'xhigh'];
const PAGES = ['Board', 'List', 'Graph', 'Analytics', 'Settings'];
const words = text => (text || '').trim().split(/\s+/).filter(Boolean).length;
const workspaceOf = t => t.workspace ?? t.cwd;
const when = ms => (ms ? new Date(ms).toLocaleString() : '');

async function api(path, method = 'GET', body) {
  const response = await fetch(path, {method, headers: {'Content-Type': 'application/json'}, body: body === undefined ? undefined : JSON.stringify(body)});
  const value = await response.json();
  if (!response.ok) throw Error(value.error || `Request failed (${response.status})`);
  return value;
}

function phase(t, state) {
  if (t.blocked) return {text: `Needs decision: ${t.failure}`, tone: 'bad'};
  if (t.stopped || state.paused) return {text: t.active ? 'Stopping' : 'Stopped', tone: 'muted'};
  if (t.active) return {text: `${t.stage === 'replan' ? 'Replanning' : t.stage === 'review' ? 'Reviewing' : 'Implementing'}`, tone: 'live'};
  if (t.stage === 'done') return {text: t.container ? 'All child tasks done' : 'Accepted', tone: 'good'};
  if (t.stage === 'replan') return {text: 'Queued for replanning', tone: 'warn'};
  return {text: t.waitingReason || (t.failure ? `Retrying: ${t.failure}` : 'Ready'), tone: t.waitingReason ? 'muted' : ''};
}

function Modal({title, close, children}) {
  const ref = useRef(null);
  useEffect(() => {
    ref.current.showModal();
  }, []);
  return (
    <dialog ref={ref} onCancel={e => { e.preventDefault(); close(); }} aria-labelledby="modal-title">
      <header><h2 id="modal-title">{title}</h2><button onClick={close} aria-label="Close">×</button></header>
      {children}
    </dialog>
  );
}

function App() {
  const [state, setState] = useState(null);
  const [offline, setOffline] = useState('');
  const [page, setPage] = useState(() => localStorage.getItem('coordinator.page') || 'Board');
  const [workspace, setWorkspace] = useState(() => localStorage.getItem('coordinator.workspace') || '');
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState(null);
  const [editor, setEditor] = useState(null);
  const [error, setError] = useState('');
  const refresh = useCallback(async () => {
    try { setState(await api('/api/state')); setOffline(''); } catch (e) { setOffline(e.message); }
  }, []);
  useEffect(() => {
    let live = true, timer;
    const poll = async () => { await refresh(); if (live) timer = setTimeout(poll, 1500); };
    poll();
    return () => { live = false; clearTimeout(timer); };
  }, [refresh]);
  async function act(path, method = 'POST', body = {actor: 'human', reason: 'User board control'}) {
    setError('');
    try { await api(path, method, body); await refresh(); return true; } catch (e) { setError(e.message); return false; }
  }
  const tasks = state?.tasks || [];
  const byId = useMemo(() => Object.fromEntries(tasks.map(t => [t.id, t])), [tasks]);
  const workspaces = [...new Set(tasks.map(workspaceOf))].sort();
  const scoped = tasks.filter(t => !workspace || workspaceOf(t) === workspace);
  const visible = scoped.filter(t => t.title.toLowerCase().includes(query.toLowerCase()));
  const go = p => { setPage(p); localStorage.setItem('coordinator.page', p); };
  const card = t => {
    const p = phase(t, state);
    return (
      <article className={`task ${p.tone}`} key={t.id}>
        <button className="task-title" onClick={() => setSelected(t.id)}>{t.title}</button>
        <div className="chips"><span>{t.attributes.lane}</span><span>P{t.attributes.priority ?? 2}</span>{t.container && <span>container · {t.children.length}</span>}{t.parent && <span>child</span>}</div>
        <p className="reason">{p.text}</p>
      </article>
    );
  };
  if (!state) return <main className="loading">{offline || 'Connecting…'}</main>;
  return (
    <div className="shell">
      <aside>
        <div className="brand">Coordinator</div>
        <label>Workspace
          <select value={workspace} onChange={e => { setWorkspace(e.target.value); localStorage.setItem('coordinator.workspace', e.target.value); }}>
            <option value="">All workspaces</option>{workspaces.map(w => <option key={w}>{w}</option>)}
          </select>
        </label>
        <nav>{PAGES.map(p => <button key={p} aria-current={page === p ? 'page' : undefined} onClick={() => go(p)}>{p}</button>)}</nav>
      </aside>
      <main>
        <header className="top">
          <h1>{page}</h1>
          <span className={`pill ${state.paused ? 'bad' : 'good'}`}>{offline ? 'Disconnected' : state.paused ? 'Board stopped' : state.holdUntil > Date.now() ? `Usage hold until ${when(state.holdUntil)}` : 'Running'}</span>
          <button onClick={() => act(`/api/${state.paused ? 'resume' : 'stop'}`)}>{state.paused ? 'Resume board' : 'Stop board'}</button>
          <button className="primary" onClick={() => setEditor({})}>New task</button>
        </header>
        {error && <p className="error" role="alert">{error}</p>}
        {state.error && <p className="error">Scheduler: {state.error}</p>}
        {['Board', 'List'].includes(page) && <div className="filters"><input type="search" placeholder="Filter by title" value={query} onChange={e => setQuery(e.target.value)} /><span>{visible.length} tasks</span></div>}
        {page === 'Board' && <div className="board">{STATUSES.map(s => <section key={s} className="column"><h2>{s} <span>{visible.filter(t => t.status === s).length}</span></h2>{visible.filter(t => t.status === s).map(card)}</section>)}</div>}
        {page === 'List' && <table className="list"><thead><tr><th>Task</th><th>Status</th><th>Lane</th><th>Blocked by</th><th>State</th></tr></thead><tbody>{visible.map(t => <tr key={t.id}><td><button className="link" onClick={() => setSelected(t.id)}>{t.title}</button></td><td>{t.status}</td><td>{t.attributes.lane}</td><td>{t.dependencies.map(d => byId[d]?.title).filter(Boolean).join(', ')}</td><td>{phase(t, state).text}</td></tr>)}</tbody></table>}
        {page === 'Graph' && <Graph tasks={scoped} byId={byId} select={setSelected} state={state} />}
        {page === 'Analytics' && <Analytics workspace={workspace} select={setSelected} />}
        {page === 'Settings' && <Settings state={state} act={act} />}
      </main>
      {selected && byId[selected] && <Detail task={byId[selected]} byId={byId} state={state} close={() => setSelected(null)} select={setSelected} act={act} edit={t => { setSelected(null); setEditor({task: t}); }} />}
      {editor && <Editor {...editor} tasks={tasks} state={state} workspace={workspace} close={() => { setEditor(null); refresh(); }} />}
    </div>
  );
}

function layout(tasks) {
  const ids = new Set(tasks.map(t => t.id));
  const prerequisites = t => [...t.dependencies, ...t.after].filter(id => ids.has(id));
  const depth = {};
  const visit = (t, trail = new Set()) => {
    if (depth[t.id] !== undefined) return depth[t.id];
    if (trail.has(t.id)) return 0;
    trail.add(t.id);
    const byId = Object.fromEntries(tasks.map(x => [x.id, x]));
    const parentDepth = t.parent && ids.has(t.parent) ? visit(byId[t.parent], trail) + 1 : 0;
    depth[t.id] = Math.max(parentDepth, ...prerequisites(t).map(id => visit(byId[id], trail) + 1), 0);
    return depth[t.id];
  };
  tasks.forEach(t => visit(t));
  const columns = [];
  tasks.forEach(t => (columns[depth[t.id]] ||= []).push(t));
  const position = {};
  columns.forEach((column, x) => column.forEach((t, y) => { position[t.id] = {x: 40 + x * 260, y: 40 + y * 86}; }));
  return {position, width: 80 + columns.length * 260, height: 80 + Math.max(1, ...columns.map(c => c.length)) * 86};
}

function Graph({tasks, byId, select, state}) {
  const [showDone, setShowDone] = useState(false);
  const shown = tasks.filter(t => showDone || t.stage !== 'done' || tasks.some(o => o.stage !== 'done' && (o.dependencies.includes(t.id) || o.after.includes(t.id))));
  const {position, width, height} = layout(shown);
  const edges = [];
  shown.forEach(t => {
    t.dependencies.forEach(d => position[d] && edges.push({from: d, to: t.id, kind: 'dependency'}));
    t.after.forEach(d => position[d] && edges.push({from: d, to: t.id, kind: 'after'}));
    if (t.parent && position[t.parent]) edges.push({from: t.parent, to: t.id, kind: 'parent'});
    t.conflicts.forEach(d => position[d] && edges.push({from: d, to: t.id, kind: 'conflict'}));
  });
  const W = 210, H = 54;
  return (
    <>
      <div className="filters">
        <label className="check"><input type="checkbox" checked={showDone} onChange={e => setShowDone(e.target.checked)} /> Show finished tasks</label>
        <span className="legend"><i className="dependency" />blocked by <i className="after" />runs after <i className="parent" />child of <i className="conflict" />conflicts</span>
      </div>
      {!shown.length && <p className="empty">No unfinished tasks in this scope.</p>}
      <div className="graph">
        <svg width={width} height={height} role="img" aria-label="Task dependency graph">
          <defs>{['dependency', 'after', 'parent'].map(k => <marker key={k} id={`arrow-${k}`} viewBox="0 0 10 10" refX="10" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path d="M0,0L10,5L0,10z" className={`head ${k}`} /></marker>)}</defs>
          {edges.map((e, i) => {
            const a = position[e.from], b = position[e.to];
            const x1 = a.x + W, y1 = a.y + H / 2, x2 = b.x, y2 = b.y + H / 2, mid = (x1 + x2) / 2;
            return <path key={i} className={`edge ${e.kind}`} d={`M${x1},${y1} C${mid},${y1} ${mid},${y2} ${x2},${y2}`} markerEnd={e.kind === 'conflict' ? undefined : `url(#arrow-${e.kind})`} />;
          })}
          {shown.map(t => {
            const p = position[t.id], tone = phase(t, state).tone;
            return (
              <g key={t.id} className={`node ${tone} ${t.status.replace(' ', '-').toLowerCase()}`} transform={`translate(${p.x},${p.y})`} onClick={() => select(t.id)} tabIndex="0" onKeyDown={e => e.key === 'Enter' && select(t.id)}>
                <rect width={W} height={H} rx="6" />
                <text x="10" y="21">{t.title.length > 30 ? `${t.title.slice(0, 29)}…` : t.title}</text>
                <text x="10" y="40" className="sub">{t.container ? `container · ${t.children.length} children` : `${t.status} · ${t.attributes.lane}`}</text>
              </g>
            );
          })}
        </svg>
      </div>
    </>
  );
}

function Detail({task: t, byId, state, close, select, act, edit}) {
  const link = id => byId[id] ? <button key={id} className="link" onClick={() => select(id)}>{byId[id].title}</button> : null;
  const relations = [['Blocked by', t.dependencies], ['Blocks', t.blocks], ['Runs after', t.after], ['Parent', t.parent ? [t.parent] : []], ['Children', t.children], ['Conflicts with', t.conflicts], ['Related', t.related]].filter(([, ids]) => ids.length);
  const control = t.stopped || t.blocked ? 'resume' : 'stop';
  return (
    <Modal title={t.title} close={close}>
      <div className="actions">
        <span className="pill">{t.status}</span><span>Stage {t.stage} · revision {t.revision}</span>
        {t.stage !== 'done' && <button onClick={() => act(`/api/tasks/${t.id}/${control}`)}>{control === 'resume' ? 'Resume' : 'Stop'}</button>}
        <button disabled={!!t.active} onClick={() => edit(t)}>Edit</button>
        {t.stage !== 'done' && !t.container && <button disabled={!!t.active} onClick={() => act(`/api/tasks/${t.id}/replan`, 'POST', {actor: 'human', reason: 'User requested a new plan'})}>Replan</button>}
        <button disabled={!!t.active} onClick={async () => { if (confirm('Archive this task?') && await act(`/api/tasks/${t.id}`, 'DELETE', {actor: 'human'})) close(); }}>Archive</button>
      </div>
      <p className={`reason ${phase(t, state).tone}`}>{phase(t, state).text}</p>
      <p>{t.outcome}</p>
      {!!t.acceptance.length && <><h3>Acceptance</h3><ol>{t.acceptance.map((c, i) => <li key={i}>{c}</li>)}</ol></>}
      <dl className="facts">
        <dt>Lane</dt><dd>{t.attributes.lane}</dd><dt>Needs</dt><dd>{t.attributes.needs.join(', ') || 'none'}</dd>
        <dt>Priority</dt><dd>{t.attributes.priority ?? 2}</dd><dt>Size</dt><dd>{t.attributes.size}</dd>
        <dt>Artifact</dt><dd>{t.attributes.artifact}{t.attributes.deploy ? ' · deploys on acceptance' : ''}</dd>
        <dt>Words</dt><dd>{t.definitionWords} / {state.versions.limits.words}</dd>
        {relations.map(([label, ids]) => <React.Fragment key={label}><dt>{label}</dt><dd className="links">{ids.map(link)}</dd></React.Fragment>)}
        <dt>Workspace</dt><dd className="path">{workspaceOf(t)}</dd>
      </dl>
      {!!t.feedback.length && <><h3>Feedback for next attempt</h3><ul>{t.feedback.map((f, i) => <li key={i}>{f}</li>)}</ul></>}
      <h3>Attempts</h3>
      <ol className="attempts">{[...t.attempts].reverse().map(a => (
        <li key={a.id}>
          <strong>{a.stage}</strong> · {a.result?.status || a.error || (a.ended ? 'ended' : 'running')} · {when(a.started)} · <a href={`/api/attempts/${a.id}/log`} target="_blank" rel="noreferrer">log</a>
          {a.result && <p>{a.result.summary}</p>}
          {a.result?.criteria && <p className="muted">Criteria: {a.result.criteria.join(', ')}</p>}
          {!!a.result?.findings?.length && <ul>{a.result.findings.map((f, i) => <li key={i}>{f}</li>)}</ul>}
        </li>
      ))}</ol>
    </Modal>
  );
}

function Editor({task, tasks, state, workspace, close}) {
  const limits = state.versions.limits;
  const [form, setForm] = useState(() => ({
    title: task?.title || '', outcome: task?.outcome || '', acceptance: [...(task?.acceptance || []), '', '', ''].slice(0, limits.criteria),
    cwd: task?.cwd || workspace || '', lane: task?.attributes.lane || 'code', needs: task?.attributes.needs || [], priority: String(task?.attributes.priority ?? 2),
    size: task?.attributes.size || 'small', artifact: task?.attributes.artifact || 'none', deploy: !!task?.attributes.deploy,
    dependencies: task?.dependencies || [], after: task?.after || [], conflicts: task?.conflicts || [], related: task?.related || [], parent: task?.parent || '',
    writable: (task?.writable || []).join('\n'), model: task?.model || '', effort: task?.effort || '',
  }));
  const [error, setError] = useState('');
  const set = (key, value) => setForm(f => ({...f, [key]: value}));
  const total = words(form.title) + words(form.outcome) + form.acceptance.reduce((n, c) => n + words(c), 0);
  const others = tasks.filter(t => t.id !== task?.id && (!workspace || workspaceOf(t) === workspace));
  const pick = key => (
    <label>{{dependencies: 'Blocked by (needs result)', after: 'Runs after (ordering only)', conflicts: 'Never runs alongside', related: 'Related'}[key]}
      <select multiple value={form[key]} onChange={e => set(key, [...e.target.selectedOptions].map(o => o.value))}>{others.map(t => <option key={t.id} value={t.id}>{t.title}</option>)}</select>
    </label>
  );
  async function submit(e) {
    e.preventDefault();
    const acceptance = form.acceptance.map(c => c.trim()).filter(Boolean);
    const body = {actor: 'human', title: form.title, outcome: form.outcome, acceptance, cwd: form.cwd,
      dependencies: form.dependencies, after: form.after, conflicts: form.conflicts, related: form.related, parent: form.parent || null,
      writable: form.writable.split('\n').map(w => w.trim()).filter(Boolean), model: form.model || null, effort: form.effort || null,
      attributes: {lane: form.lane, needs: form.needs, priority: Number(form.priority), size: form.size, artifact: form.artifact, deploy: form.deploy}};
    try { await api(task ? `/api/tasks/${task.id}` : '/api/tasks', task ? 'PATCH' : 'POST', body); close(); } catch (err) { setError(err.message); }
  }
  return (
    <Modal title={task ? 'Edit task' : 'New task'} close={close}>
      <form onSubmit={submit} className="editor">
        <p className={total > limits.words ? 'error' : 'muted'}>{total} / {limits.words} words · at most {limits.criteria} criteria · leave criteria empty to make a container for child tasks</p>
        <label>Title<input required value={form.title} onChange={e => set('title', e.target.value)} /></label>
        <label>Outcome<textarea required rows="3" value={form.outcome} onChange={e => set('outcome', e.target.value)} /></label>
        {form.acceptance.map((c, i) => <label key={i}>Criterion {i + 1}<input value={c} onChange={e => set('acceptance', form.acceptance.map((x, j) => (j === i ? e.target.value : x)))} /></label>)}
        <label>Working directory<input required value={form.cwd} onChange={e => set('cwd', e.target.value)} /></label>
        <div className="row">
          <label>Lane<select value={form.lane} onChange={e => set('lane', e.target.value)}>{LANES.map(v => <option key={v}>{v}</option>)}</select></label>
          <label>Priority<select value={form.priority} onChange={e => set('priority', e.target.value)}>{[0, 1, 2, 3].map(v => <option key={v}>{v}</option>)}</select></label>
          <label>Size<select value={form.size} onChange={e => set('size', e.target.value)}>{SIZES.map(v => <option key={v}>{v}</option>)}</select></label>
          <label>Artifact<select value={form.artifact} onChange={e => set('artifact', e.target.value)}>{ARTIFACTS.map(v => <option key={v}>{v}</option>)}</select></label>
        </div>
        <fieldset><legend>Worker needs (available: {state.config.capabilities.join(', ') || 'none'})</legend>{NEEDS.map(n => <label className="check" key={n}><input type="checkbox" checked={form.needs.includes(n)} onChange={e => set('needs', e.target.checked ? [...form.needs, n] : form.needs.filter(x => x !== n))} /> {n}</label>)}
          <label className="check"><input type="checkbox" checked={form.deploy} onChange={e => set('deploy', e.target.checked)} /> Deploy candidate on acceptance</label></fieldset>
        <label>Parent container<select value={form.parent} onChange={e => set('parent', e.target.value)}><option value="">None</option>{others.filter(t => t.container && t.stage !== 'done').map(t => <option key={t.id} value={t.id}>{t.title}</option>)}</select></label>
        <div className="row">{pick('dependencies')}{pick('after')}</div>
        <div className="row">{pick('conflicts')}{pick('related')}</div>
        <label>Writable paths (one per line)<textarea rows="2" value={form.writable} onChange={e => set('writable', e.target.value)} /></label>
        <div className="row">
          <label>Model<input value={form.model} onChange={e => set('model', e.target.value)} placeholder="Default" /></label>
          <label>Effort<select value={form.effort} onChange={e => set('effort', e.target.value)}>{EFFORTS.map(v => <option key={v} value={v}>{v || 'Default'}</option>)}</select></label>
        </div>
        {error && <p className="error" role="alert">{error}</p>}
        <footer><button type="button" onClick={close}>Cancel</button><button className="primary">{task ? 'Save' : 'Create'}</button></footer>
      </form>
    </Modal>
  );
}

function Analytics({workspace, select}) {
  const [group, setGroup] = useState('lane');
  const [report, setReport] = useState(null);
  const [error, setError] = useState('');
  useEffect(() => {
    api('/api/analytics', 'POST', {groupBy: group, filters: workspace ? {workspace} : {}}).then(setReport).catch(e => setError(e.message));
  }, [group, workspace]);
  const n = v => (v?.mean == null ? '—' : v.mean.toFixed(2));
  return (
    <>
      <div className="filters"><label>Group by <select value={group} onChange={e => setGroup(e.target.value)}>{['lane', 'size', 'priority', 'needs', 'artifact', 'criteria', 'relations', 'container', 'workspace'].map(g => <option key={g}>{g}</option>)}</select></label></div>
      {error && <p className="error">{error}</p>}
      {report && <table className="list"><thead><tr><th>{group}</th><th>Tasks</th><th>First pass</th><th>Attempts</th><th>Rejections</th><th>Replans</th><th>Lead (h)</th></tr></thead><tbody>
        {Object.entries(report.groups).map(([k, g]) => <tr key={k}><td>{k}</td><td>{g.tasks}</td><td>{n(g.firstPass)}</td><td>{n(g.attempts)}</td><td>{n(g.rejections)}</td><td>{n(g.replans)}</td><td>{g.leadMs.mean == null ? '—' : (g.leadMs.mean / 3600000).toFixed(1)}</td></tr>)}
      </tbody></table>}
      {report && <p className="muted">{report.definitions.firstPass}. Groups are associations, not causes.</p>}
    </>
  );
}

function Settings({state, act}) {
  const [text, setText] = useState(() => JSON.stringify(state.config, null, 2));
  const [message, setMessage] = useState('');
  return (
    <form onSubmit={async e => { e.preventDefault(); try { if (await act('/api/config', 'PUT', JSON.parse(text))) setMessage('Saved.'); } catch (err) { setMessage(err.message); } }}>
      <p>Worker commands for each stage, the capabilities those workers provide, and an optional notify command that receives held, usage-limit and deploy events as JSON on stdin.</p>
      <textarea className="code" rows="18" value={text} onChange={e => setText(e.target.value)} />
      <p role="status">{message}</p>
      <button className="primary">Save configuration</button>
      <h3>Deployment</h3><pre>{JSON.stringify({live: state.deployment, last: state.deploymentEvent}, null, 2)}</pre>
    </form>
  );
}

createRoot(document.getElementById('root')).render(<App />);
