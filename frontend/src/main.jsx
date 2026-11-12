import React, {useState, useEffect, useRef, useCallback, useMemo} from 'react';
import {createRoot} from 'react-dom/client';
import Graph from './Graph';
import {BorderBeam, AnimatedCount} from './Activity';
import './style.css';

const readPreference = key => { try { return localStorage.getItem(key); } catch { return null; } };
const savePreference = (key, value) => { try { localStorage.setItem(key, value); } catch {} };

const STATUSES = ['To do', 'In progress', 'Review', 'Done'];
const LANES = ['code', 'ui', 'data', 'docs', 'audit', 'cleanup'];
const NEEDS = ['network', 'browser', 'process', 'gpu'];
const SIZES = ['small', 'medium', 'large'];
const ARTIFACTS = ['none', 'candidate', 'report'];
const EFFORTS = ['', 'minimal', 'low', 'medium', 'high', 'xhigh'];
const PAGES = ['Board', 'List', 'Graph', 'Analytics', 'Settings'];
const words = text => (text || '').trim().split(/\s+/).filter(Boolean).length;
const workspaceOf = t => t.workspace ?? t.cwd;
const pathKey = p => (p || '').replace(/^\\\\\?\\/, '').replace(/[\\/]+$/, '').toLowerCase();
const limitsFor = (state, path) => ({...state.versions.limits, ...(Object.entries(state.config.limits || {}).find(([k]) => pathKey(k) === pathKey(path))?.[1] || {})});
const dependencyReasons = state => state.versions.dependencyReasons || ['uses_artifact', 'uses_api', 'uses_schema', 'uses_data'];
const dependencyTarget = d => typeof d === 'string' ? d : d.target;
const dependencyError = (links, reasons) => links.some(d => !dependencyTarget(d)) ? 'Choose a task for every dependency or remove the dependency.' : links.some(d => !reasons.includes(d.reason)) ? 'Choose an allowed reason for every dependency or remove the dependency.' : '';
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
    const opener = document.activeElement;
    const dialog = ref.current;
    dialog.showModal();
    return () => {dialog.close(); if (opener?.isConnected) opener.focus();};
  }, []);
  return (
    <dialog ref={ref} onCancel={e => { e.preventDefault(); close(); }} aria-labelledby="modal-title">
      <header><h2 id="modal-title">{title}</h2><button onClick={close} aria-label="Close">×</button></header>
      {children}
    </dialog>
  );
}

const categoryLabels = {languages: 'Languages', functions: 'Functional responsibilities'};
const categoryName = value => ({cpp: 'C++', csharp: 'C#', javascript: 'JavaScript', typescript: 'TypeScript', powershell: 'PowerShell', api: 'API', ui: 'UI', css: 'CSS', html: 'HTML', sql: 'SQL'}[value] || value.charAt(0).toUpperCase() + value.slice(1));
const categoryOptions = (state, key) => state?.versions.classifications?.[key]?.items.enum || [];
const categoryText = (attributes, key) => attributes[key]?.length ? attributes[key].map(categoryName).join(', ') : 'Uncategorized';
const classificationError = (attributes, state) => Object.keys(categoryLabels).some(key => (attributes[key] || []).some(value => !categoryOptions(state, key).includes(value))) ? 'Remove unsupported classifications before saving.' : '';
function Classifications({attributes, state, change}) {
  return <section className="classifications" aria-label="Task classifications"><p className="muted">Select every applicable language and function. Empty selections are uncategorized. Classifications do not change lane or execution capabilities.</p>
    {Object.entries(categoryLabels).map(([key, label]) => <fieldset key={key}><legend>{label}</legend>
      {[...new Set([...categoryOptions(state, key), ...(attributes[key] || [])])].map(value => <label className="check" key={value}><input type="checkbox" checked={(attributes[key] || []).includes(value)} onChange={e => change(key, e.target.checked ? [...(attributes[key] || []), value] : attributes[key].filter(v => v !== value))} />{categoryName(value)}{!categoryOptions(state, key).includes(value) && ' (unsupported — remove)'}</label>)}
      {!attributes[key]?.length && <span className="muted">Uncategorized</span>}
    </fieldset>)}
  </section>;
}

function App() {
  const [theme, setTheme] = useState(() => {
    const initial = readPreference('coordinator.theme') === 'light' ? 'light' : 'dark';
    document.documentElement.dataset.theme = initial;
    document.documentElement.style.background = initial === 'dark' ? '#191b20' : '#f6f7f9';
    document.documentElement.style.colorScheme = initial;
    return initial;
  });
  const toggleTheme = () => {
    const next = theme === 'dark' ? 'light' : 'dark';
    document.documentElement.dataset.theme = next;
    document.documentElement.style.background = next === 'dark' ? '#191b20' : '#f6f7f9';
    document.documentElement.style.colorScheme = next;
    setTheme(next);
    savePreference('coordinator.theme', next);
  };
  const themeToggle = <button className="theme-toggle" onClick={toggleTheme} aria-label={`Switch to ${theme === 'dark' ? 'light' : 'dark'} theme`} title={`Switch to ${theme === 'dark' ? 'light' : 'dark'} theme`}><span aria-hidden="true">{theme === 'dark' ? '☀' : '☾'}</span><span>{theme === 'dark' ? 'Light' : 'Dark'}</span></button>;
  const [state, setState] = useState(null);
  const [offline, setOffline] = useState('');
  const [page, setPage] = useState(() => readPreference('coordinator.page') || 'Board');
  const [workspace, setWorkspace] = useState(() => readPreference('coordinator.workspace') || '');
  const [query, setQuery] = useState('');
  const [filters, setFilters] = useState({languages: '', functions: '', lane: '', needs: ''});
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
  const visible = scoped.filter(t => t.title.toLowerCase().includes(query.toLowerCase()) && Object.entries(filters).every(([key, value]) => !value || (key === 'lane' ? t.attributes.lane === value : value === 'uncategorized' ? !t.attributes[key]?.length : (t.attributes[key] || []).includes(value))));
  const go = p => { setPage(p); savePreference('coordinator.page', p); };
  const card = t => {
    const p = phase(t, state);
    return (
      <article className={`task ${p.tone}`} key={t.id}>
        {p.tone === 'live' && !offline && <BorderBeam />}
        <button className="task-title" onClick={() => setSelected(t.id)}>{t.title}</button>
        <div className="chips"><span>Lane: {t.attributes.lane}</span><span>Languages: {categoryText(t.attributes, 'languages')}</span><span>Functions: {categoryText(t.attributes, 'functions')}</span><span>P{t.attributes.priority ?? 2}</span>{t.container && <span>container · {t.children.length}</span>}{t.parent && <span>child</span>}</div>
        <p className={`reason ${p.tone}`}>{p.text}</p>
      </article>
    );
  };
  if (!state) return <main><header className="top"><h1>Coordinator</h1>{themeToggle}</header><p role="status">{offline || 'Connecting…'}</p></main>;
  return (
    <div className="shell">
      <aside>
        <div className="brand">Coordinator</div>
        <label>Workspace
          <select aria-label="Workspace" value={workspace} onChange={e => { setWorkspace(e.target.value); savePreference('coordinator.workspace', e.target.value); }}>
            <option value="">All workspaces</option>{workspaces.map(w => <option key={w}>{w}</option>)}
          </select>
        </label>
        <nav>{PAGES.map(p => <button key={p} aria-current={page === p ? 'page' : undefined} onClick={() => go(p)}>{p}</button>)}</nav>
      </aside>
      <main>
        <header className="top">
          <h1>{page}</h1>{themeToggle}
          <span className={`pill ${offline || state.paused ? 'bad' : state.holdUntil > Date.now() ? 'warn' : 'good'}`}>{offline ? 'Disconnected' : state.paused ? 'Board stopped' : state.holdUntil > Date.now() ? `Usage hold until ${when(state.holdUntil)}` : 'Running'}</span>
          <button onClick={() => act(`/api/${state.paused ? 'resume' : 'stop'}`)}>{state.paused ? 'Resume board' : 'Stop board'}</button>
          <button onClick={() => setEditor({})}>New task</button><button className="primary" onClick={() => setEditor({grouped: true})}>New grouped work</button>
        </header>
        {error && <p className="error" role="alert">{error}</p>}
        {state.error && <p className="error">Scheduler: {state.error}</p>}
        {['Board', 'List'].includes(page) && <div className="filters"><input type="search" placeholder="Filter by title" value={query} onChange={e => setQuery(e.target.value)} />{Object.entries({...categoryLabels, lane: 'Lane', needs: 'Execution capability'}).map(([key, label]) => <label key={key}>{label}<select aria-label={`Filter ${label.toLowerCase()}`} value={filters[key]} onChange={e => setFilters(f => ({...f, [key]: e.target.value}))}><option value="">All</option>{key !== 'lane' && <option value="uncategorized">{key === 'needs' ? 'No capabilities required' : 'Uncategorized'}</option>}{(key === 'lane' ? LANES : key === 'needs' ? NEEDS : categoryOptions(state, key)).map(v => <option key={v} value={v}>{categoryName(v)}</option>)}</select></label>)}<button onClick={() => {setQuery(''); setFilters({languages: '', functions: '', lane: '', needs: ''});}}>Clear filters</button><span><AnimatedCount value={visible.length} /> tasks</span><small>All selected filters must match.</small></div>}
        {page === 'Board' && <div className="board page-enter">{STATUSES.map(s => <section key={s} className="column"><h2>{s} <AnimatedCount value={visible.filter(t => t.status === s).length} /></h2>{visible.filter(t => t.status === s).map(card)}</section>)}</div>}
        {page === 'List' && <table className="list page-enter"><thead><tr><th>Task</th><th>Status</th><th>Lane</th><th>Languages</th><th>Functions</th><th>Execution capabilities</th><th>Blocked by</th><th>State</th></tr></thead><tbody>{visible.map(t => <tr key={t.id}><td><button className="link" onClick={() => setSelected(t.id)}>{t.title}</button></td><td>{t.status}</td><td>{t.attributes.lane}</td><td>{categoryText(t.attributes, 'languages')}</td><td>{categoryText(t.attributes, 'functions')}</td><td>{t.attributes.needs.join(', ') || 'none'}</td><td>{t.dependencies.map(d => byId[typeof d === 'string' ? d : d.target]?.title).filter(Boolean).join(', ')}</td><td className={`reason ${phase(t, state).tone}`}>{phase(t, state).text}</td></tr>)}</tbody></table>}
        {page === 'Graph' && <Graph key={workspace} activityEnabled={!offline && !state.paused} workspace={workspace} tasks={scoped} allTasks={tasks} select={setSelected} />}
        {page === 'Analytics' && <Analytics workspace={workspace} select={setSelected} />}
        {page === 'Settings' && <Settings state={state} act={act} />}
      </main>
      {selected && byId[selected] && <Detail task={byId[selected]} byId={byId} state={state} close={() => setSelected(null)} select={setSelected} act={act} edit={t => { setSelected(null); setEditor({task: t}); }} />}
      {editor && !editor.grouped && <Editor {...editor} tasks={tasks} state={state} workspace={workspace} close={() => { setEditor(null); refresh(); }} />}
      {editor?.grouped && <GroupEditor tasks={tasks} state={state} workspace={workspace} close={() => { setEditor(null); refresh(); }} />}
    </div>
  );
}

function Detail({task: t, byId, state, close, select, act, edit}) {
  const reasons = dependencyReasons(state);
  const typed = d => ({target: dependencyTarget(d), reason: d.reason});
  const link = relation => {
    const id = dependencyTarget(relation);
    const suffix = typeof relation === 'object' ? ` · ${reasons.includes(relation.reason) ? relation.reason.replaceAll('_', ' ') : `Unresolved reason${relation.reason ? `: ${relation.reason}` : ''}`}` : '';
    return byId[id] ? <button key={id} className="link" onClick={() => select(id)}>{byId[id].title}{suffix}</button> : <span key={id}>Unavailable task: {id}{suffix}</span>;
  };
  const all = Object.values(byId);
  const blocks = all.flatMap(task => task.dependencies.filter(d => dependencyTarget(d) === t.id).map(d => ({target: task.id, reason: d.reason})));
  const children = [...new Set([...t.children, ...all.filter(task => task.parent === t.id).map(task => task.id)])];
  const relations = [['Dependencies', t.dependencies.map(typed)], ['Required by', blocks], ['Runs after (ordering only)', t.after], ['Parent', t.parent ? [t.parent] : []], ['Children', children], ['Conflicts with', t.conflicts], ['Related', t.related]].filter(([, ids]) => ids.length);
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
        <dt>Grouping</dt><dd>{t.parent ? 'Child of parent container' : t.standalone ? 'Standalone' : 'Unresolved — choose Parent container or Standalone in Edit'}</dd><dt>Lane</dt><dd>{t.attributes.lane}</dd><dt>Languages</dt><dd>{categoryText(t.attributes, 'languages')}</dd><dt>Functional responsibilities</dt><dd>{categoryText(t.attributes, 'functions')}</dd><dt>Execution capabilities</dt><dd>{t.attributes.needs.join(', ') || 'none'}</dd>
        <dt>Priority</dt><dd>{t.attributes.priority ?? 2}</dd><dt>Size</dt><dd>{t.attributes.size}</dd>
        <dt>Artifact</dt><dd>{t.attributes.artifact}{t.attributes.deploy ? ' · deploys on acceptance' : ''}</dd>
        <dt>Words</dt><dd>{t.definitionWords} / {limitsFor(state, workspaceOf(t)).words}</dd>
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
  const [form, setForm] = useState(() => ({
    title: task?.title || '', outcome: task?.outcome || '', acceptance: Array.from({length: Math.max(limitsFor(state, task ? workspaceOf(task) : workspace).criteria, task?.acceptance.length || 0)}, (_, i) => task?.acceptance[i] || ''),
    cwd: task?.cwd || workspace || '', lane: task?.attributes.lane || 'code', needs: task?.attributes.needs || [], priority: String(task?.attributes.priority ?? 2),
    languages: task?.attributes.languages || [], functions: task?.attributes.functions || [],
    size: task?.attributes.size || 'small', artifact: task?.attributes.artifact || 'none', deploy: !!task?.attributes.deploy,
    dependencies: task?.dependencies || [], after: task?.after || [], conflicts: task?.conflicts || [], related: task?.related || [], parent: task?.parent || '',
    writable: (task?.writable || []).join('\n'), model: task?.model || '', effort: task?.effort || '',
  }));
  const [error, setError] = useState('');
  const [grouping, setGrouping] = useState(task?.parent ? 'parent' : task?.standalone ? 'standalone' : '');
  const [groupingChanged, setGroupingChanged] = useState(false);
  const [busy, setBusy] = useState(false);
  const set = (key, value) => setForm(f => ({...f, [key]: value}));
  const total = words(form.title) + words(form.outcome) + form.acceptance.reduce((n, c) => n + words(c), 0);
  const limits = limitsFor(state, form.cwd);
  const rows = Array.from({length: Math.max(limits.criteria, form.acceptance.length)}, (_, i) => form.acceptance[i] || '');
  const others = tasks.filter(t => t.id !== task?.id && (!workspace || workspaceOf(t) === workspace));
  const pick = key => (
    <label>{{dependencies: 'Blocked by (needs result)', after: 'Runs after (ordering only)', conflicts: 'Never runs alongside', related: 'Related'}[key]}
      <select aria-label={{after: 'Runs after (ordering only)', conflicts: 'Never runs alongside', related: 'Related'}[key]} multiple value={form[key]} onChange={e => set(key, [...e.target.selectedOptions].map(o => o.value))}>{others.map(t => <option key={t.id} value={t.id}>{t.title}</option>)}</select>
    </label>
  );
  async function submit(e) {
    e.preventDefault();
    if (busy) return;
    const invalid = classificationError(form, state) || dependencyError(form.dependencies, dependencyReasons(state));
    if (invalid) {setError(invalid); return;}
    if ((!task || groupingChanged) && (!grouping || (grouping === 'parent' && !form.parent))) {setError('Choose Parent container or Standalone explicitly.'); return;}
    const acceptance = form.acceptance.map(c => c.trim()).filter(Boolean);
    const body = {actor: 'human', title: form.title, outcome: form.outcome, acceptance, cwd: form.cwd,
      dependencies: form.dependencies, after: form.after, conflicts: form.conflicts, related: form.related,
      writable: form.writable.split('\n').map(w => w.trim()).filter(Boolean), model: form.model || null, effort: form.effort || null,
      attributes: {languages: form.languages, functions: form.functions, lane: form.lane, needs: form.needs, priority: Number(form.priority), size: form.size, artifact: form.artifact, deploy: form.deploy}};
    if (!task || groupingChanged) Object.assign(body, grouping === 'parent' ? {parent: form.parent, standalone: false} : {parent: null, standalone: true});
    setBusy(true);
    try { await api(task ? `/api/tasks/${task.id}` : '/api/tasks', task ? 'PATCH' : 'POST', body); close(); } catch (err) { setError(err.message); } finally { setBusy(false); }
  }
  return (
    <Modal title={task ? 'Edit task' : 'New task'} close={close}>
      <form onSubmit={submit} className="editor">
        <p className={total > limits.words ? 'error' : 'muted'}>{total} / {limits.words} words · at most {limits.criteria} criteria · leave criteria empty to make a container for child tasks</p>
        <label>Title<input required value={form.title} onChange={e => set('title', e.target.value)} /></label>
        <label>Outcome<textarea aria-label="Outcome" required rows="3" value={form.outcome} onChange={e => set('outcome', e.target.value)} /></label>
        {rows.map((c, i) => <label key={i}>Criterion {i + 1}<input value={c} onChange={e => set('acceptance', rows.map((x, j) => (j === i ? e.target.value : x)))} /></label>)}
        <label>Working directory<input required value={form.cwd} onChange={e => set('cwd', e.target.value)} /></label>
        <div className="row">
          <label>Lane<select value={form.lane} onChange={e => set('lane', e.target.value)}>{LANES.map(v => <option key={v}>{v}</option>)}</select></label>
          <label>Priority<select value={form.priority} onChange={e => set('priority', e.target.value)}>{[0, 1, 2, 3].map(v => <option key={v}>{v}</option>)}</select></label>
          <label>Size<select value={form.size} onChange={e => set('size', e.target.value)}>{SIZES.map(v => <option key={v}>{v}</option>)}</select></label>
          <label>Artifact<select value={form.artifact} onChange={e => set('artifact', e.target.value)}>{ARTIFACTS.map(v => <option key={v}>{v}</option>)}</select></label>
        </div>
        <Classifications attributes={form} state={state} change={set} />
        <fieldset><legend>Execution capabilities (available: {state.config.capabilities.join(', ') || 'none'})</legend>{NEEDS.map(n => <label className="check" key={n}><input type="checkbox" checked={form.needs.includes(n)} onChange={e => set('needs', e.target.checked ? [...form.needs, n] : form.needs.filter(x => x !== n))} /> {n}</label>)}
          <label className="check"><input type="checkbox" checked={form.deploy} onChange={e => set('deploy', e.target.checked)} /> Deploy candidate on acceptance</label></fieldset>
        <Grouping value={grouping} parent={form.parent} tasks={others} required={!task || groupingChanged} change={v => {setGrouping(v); setGroupingChanged(true);}} setParent={v => {set('parent', v); setGroupingChanged(true);}} />
        <Dependencies value={form.dependencies} change={v => set('dependencies', v)} options={others.map(t => ({value: t.id, label: t.title}))} reasons={dependencyReasons(state)} invalid={setError} />
        <div className="row">{pick('after')}</div>
        <div className="row">{pick('conflicts')}{pick('related')}</div>
        <label>Writable paths (one per line)<textarea rows="2" value={form.writable} onChange={e => set('writable', e.target.value)} /></label>
        <div className="row">
          <label>Model<input value={form.model} onChange={e => set('model', e.target.value)} placeholder="Default" /></label>
          <label>Effort<select value={form.effort} onChange={e => set('effort', e.target.value)}>{EFFORTS.map(v => <option key={v} value={v}>{v || 'Default'}</option>)}</select></label>
        </div>
        {error && <p className="error" role="alert">{error}</p>}
        <footer><button type="button" onClick={close}>Cancel</button><button className="primary" disabled={busy}>{task ? 'Save' : 'Create'}</button></footer>
      </form>
    </Modal>
  );
}

function Grouping({value, parent, tasks, required = true, change, setParent}) {
  return <fieldset><legend>Grouping</legend>
    <label>Grouping choice<select required={required} value={value} onChange={e => change(e.target.value)}><option value="">Choose grouping explicitly</option><option value="parent">Parent container</option><option value="standalone">Standalone</option></select></label>
    {!value && <p className="muted">Grouping is unresolved. Existing tasks keep this state until you choose.</p>}
    {value === 'parent' && <label>Parent container<select aria-label="Parent container" required value={parent} onChange={e => setParent(e.target.value)}><option value="">Choose parent container</option>{tasks.filter(t => t.container && (t.stage !== 'done' || t.id === parent)).map(t => <option key={t.id} value={t.id}>{t.title}</option>)}</select></label>}
  </fieldset>;
}

function Dependencies({value, change, options, reasons, invalid}) {
  const links = value.map(d => typeof d === 'string' ? {target: d, reason: ''} : d);
  const update = (i, patch) => change(links.map((d, j) => i === j ? {...d, ...patch} : d));
  return <fieldset onInvalid={() => invalid(dependencyError(links, reasons))}><legend>Dependencies</legend>{links.map((d, i) => <div className="row" key={i}>
    <label>Dependency target<select required value={d.target} onChange={e => update(i, {target: e.target.value})}><option value="">Choose task</option>{!options.some(o => o.value === d.target) && d.target && <option value={d.target}>Unavailable task: {d.target}</option>}{options.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}</select></label>
    <label>Dependency reason<select required value={reasons.includes(d.reason) ? d.reason : ''} onChange={e => update(i, {reason: e.target.value})}><option value="">{d.target ? `Unresolved${d.reason ? `: ${d.reason}` : ''} — choose reason` : 'Choose reason'}</option>{reasons.map(r => <option key={r} value={r}>{r.replaceAll('_', ' ')}</option>)}</select></label>
    <button type="button" onClick={() => change(links.filter((_, j) => i !== j))}>Remove dependency</button>
  </div>)}<button type="button" onClick={() => change([...links, {target: '', reason: ''}])}>Add dependency</button></fieldset>;
}

function GroupEditor({tasks, state, workspace, close}) {
  const newChild = () => ({localKey: crypto.randomUUID(), title: '', outcome: '', acceptance: Array.from({length: limitsFor(state, workspace).criteria}, () => ''), dependencies: [], after: [], attributes: {lane: 'code', needs: [], languages: [], functions: []}});
  const [container, setContainer] = useState({title: '', outcome: '', cwd: workspace || '', acceptance: [], attributes: {languages: [], functions: []}});
  const [children, setChildren] = useState(() => [newChild()]);
  const [grouping, setGrouping] = useState('');
  const [parent, setParent] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const replay = useRef(null);
  const set = (key, value) => setContainer(c => ({...c, [key]: value}));
  const update = (key, patch) => setChildren(cs => cs.map(c => c.localKey === key ? {...c, ...patch} : c));
  const reference = value => value.startsWith('local:') ? {local: value.slice(6)} : value.slice(3);
  async function submit(e) {
    e.preventDefault();
    if (busy) return;
    if (!grouping || (grouping === 'parent' && !parent)) {setError('Choose Parent container or Standalone explicitly.'); return;}
    if (!children.length) {setError('Add at least one child task.'); return;}
    const invalid = classificationError(container.attributes, state) || children.map(c => classificationError(c.attributes, state) || dependencyError(c.dependencies, dependencyReasons(state))).find(Boolean);
    if (invalid) {setError(invalid); return;}
    const body = {actor: 'human', container: {...container, ...(grouping === 'parent' ? {parent, standalone: false} : {standalone: true})}, children: children.map(c => ({...c, acceptance: c.acceptance.map(v => v.trim()).filter(Boolean), dependencies: c.dependencies.map(d => ({...d, target: reference(d.target)})), after: c.after.map(reference)}))};
    if (body.children.some(c => !c.acceptance.length)) {setError('Every child needs an acceptance criterion.'); return;}
    const signature = JSON.stringify(body);
    if (replay.current?.signature !== signature) replay.current = {signature, requestKey: crypto.randomUUID()};
    setBusy(true); setError('');
    try {await api('/api/tasks/batch', 'POST', {...body, requestKey: replay.current.requestKey}); close();}
    catch (err) {setError(err.message);}
    finally {setBusy(false);}
  }
  return <Modal title="New grouped work" close={close}><form className="editor" onSubmit={submit}>
    <p>Define one container and its children. Create the whole group together.</p>
    <label>Container title<input required value={container.title} onChange={e => set('title', e.target.value)} /></label>
    <label>Container outcome<textarea required value={container.outcome} onChange={e => set('outcome', e.target.value)} /></label>
    <label>Working directory<input required value={container.cwd} onChange={e => set('cwd', e.target.value)} /></label>
    <Classifications attributes={container.attributes} state={state} change={(key, value) => set('attributes', {...container.attributes, [key]: value})} />
    <Grouping value={grouping} parent={parent} tasks={tasks} change={setGrouping} setParent={setParent} />
    {children.map((c, i) => {
      const options = [...children.filter(other => other.localKey !== c.localKey).map(other => ({value: `local:${other.localKey}`, label: `Sibling: ${other.title || 'Untitled child'}`})), ...tasks.filter(t => workspaceOf(t) === container.cwd).map(t => ({value: `id:${t.id}`, label: t.title}))];
      return <fieldset key={c.localKey} aria-label={`Child ${i + 1}`}><legend>Child {i + 1}</legend>
        <label>Child title<input required value={c.title} onChange={e => update(c.localKey, {title: e.target.value})} /></label>
        <label>Child outcome<textarea required value={c.outcome} onChange={e => update(c.localKey, {outcome: e.target.value})} /></label>
        {c.acceptance.map((v, j) => <label key={j}>Criterion {j + 1}<input value={v} onChange={e => update(c.localKey, {acceptance: c.acceptance.map((a, k) => k === j ? e.target.value : a)})} /></label>)}
        <label>Lane<select value={c.attributes.lane} onChange={e => update(c.localKey, {attributes: {...c.attributes, lane: e.target.value}})}>{LANES.map(v => <option key={v}>{v}</option>)}</select></label>
        <Classifications attributes={c.attributes} state={state} change={(key, value) => update(c.localKey, {attributes: {...c.attributes, [key]: value}})} />
        <Dependencies value={c.dependencies} change={dependencies => update(c.localKey, {dependencies})} options={options} reasons={dependencyReasons(state)} invalid={setError} />
        <label>Runs after (ordering only)<select multiple value={c.after} onChange={e => update(c.localKey, {after: [...e.target.selectedOptions].map(o => o.value)})}>{options.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}</select></label>
        <button type="button" onClick={() => setChildren(cs => cs.filter(other => other.localKey !== c.localKey).map(other => ({...other, dependencies: other.dependencies.filter(d => d.target !== `local:${c.localKey}`), after: other.after.filter(v => v !== `local:${c.localKey}`)})))}>Remove child</button>
      </fieldset>;
    })}
    <button type="button" onClick={() => setChildren(cs => [...cs, newChild()])}>Add child</button>
    {error && <p className="error" role="alert">{error}</p>}
    <footer><button type="button" disabled={busy} onClick={close}>Cancel</button><button className="primary" disabled={busy}>Create group</button></footer>
  </form></Modal>;
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
      {report && <table className="list page-enter"><thead><tr><th>{group}</th><th>Tasks</th><th>First pass</th><th>Attempts</th><th>Rejections</th><th>Replans</th><th>Lead (h)</th></tr></thead><tbody>
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
