import React, {useState, useEffect, useMemo, useRef} from 'react';
import {duration, within, pathKey} from './fields';

const FILTERS = [['action', 'Board actions', true], ['reject', 'Rejected calls', true], ['msg', 'Messages', true], ['tool', 'Other tools', false], ['think', 'Thinking', false]];
const ACTION_WORDS = {created: 'Created', edited: 'Edited', replan: 'Asked to replan', stopped: 'Stopped', resumed: 'Resumed', deleted: 'Deleted'};
const KIND_WORDS = {initial: 'Initial', 'follow-up': 'Follow-up', wake: 'Coordinator wake-up'};
const STATUS_TONE = {running: 'live', waiting: 'good', stopped: ''};
const money = n => n == null ? '—' : `$${n.toFixed(2)}`;
const clock = ms => ms ? new Date(ms).toLocaleTimeString([], {hour: '2-digit', minute: '2-digit'}) : '';
const firstLine = text => (text || '').split('\n').find(l => l.trim()) || '';
const folder = path => (path || '').replace(/^\\\\\?\\/, '').split(/[\\/]/).filter(Boolean).at(-1) || path;
const plural = (n, word) => `${n} ${word}${n === 1 ? '' : 's'}`;

const command = text => firstLine((text || '').replace(/^\s*cd\s+("[^"]*"|'[^']*'|\S+)\s*(;|&&)\s*/, '')) || firstLine(text);
function describe(block) {
  const input = block.input || {};
  if (input.command) return command(input.command);
  if (input.file_path) return `${block.name} ${input.file_path}`;
  if (input.pattern) return `${block.name} ${input.pattern}`;
  if (input.description) return `${block.name}: ${input.description}`;
  return block.name;
}
const resultText = content => typeof content === 'string' ? content : Array.isArray(content) ? content.map(c => c.text || '').join('\n') : '';

export function parseTurns(lines, firstReason) {
  const turns = [], tools = {};
  let turn = null, wake = null, at = null, spent = 0;
  const open = reason => { turn = {reason, steps: [], start: at, end: at, cost: null, duration: null, summary: '', done: false}; turns.push(turn); };
  const step = s => { if (!turn) open(firstReason); turn.steps.push({at, ...s}); return turn.steps.at(-1); };
  for (const e of lines) {
    at = e.coordinatorAt ?? at;
    if ((e.type === 'system' && e.subtype === 'init') || e.type === 'turn.started') {
      if (!turn || turn.done) open(turns.length ? (wake ? `Woke: ${wake}` : 'Continued') : firstReason);
      wake = null;
      continue;
    }
    if (e.type === 'system' && e.subtype === 'task_notification') { wake = `background task ${e.status || 'changed'}: ${firstLine(e.summary)}`; continue; }
    if (turn && at != null) { turn.start ??= at; turn.end = at; }
    if (e.type === 'assistant') {
      for (const b of Array.isArray(e.message?.content) ? e.message.content : []) {
        if (b.type === 'text' && b.text?.trim()) step({kind: 'msg', text: b.text});
        else if (b.type === 'thinking' && b.thinking?.trim()) step({kind: 'think', text: b.thinking});
        else if (b.type === 'tool_use') tools[b.id] = step({kind: 'tool', text: describe(b), full: b.input?.command || JSON.stringify(b.input, null, 2)});
      }
    } else if (e.type === 'user') {
      for (const b of Array.isArray(e.message?.content) ? e.message.content : []) {
        if (b.type === 'tool_result' && tools[b.tool_use_id]) Object.assign(tools[b.tool_use_id], {output: resultText(b.content), failed: !!b.is_error});
      }
    } else if (e.type === 'item.completed') {
      const item = e.item || {};
      if (item.type === 'agent_message') step({kind: 'msg', text: item.text});
      else if (item.type === 'reasoning' && item.text?.trim()) step({kind: 'think', text: item.text});
      else if (item.type === 'command_execution') step({kind: 'tool', text: command(item.command), full: item.command, output: item.aggregated_output, failed: item.exit_code ? true : false});
      else if (item.type) step({kind: 'tool', text: item.type});
    } else if (e.type === 'result' || e.type === 'turn.completed') {
      if (!turn) open(firstReason);
      const total = e.total_cost_usd;
      const last = turn.steps.at(-1);
      if (e.result && last?.kind === 'msg' && last.text.trim() === e.result.trim()) turn.steps.pop();
      Object.assign(turn, {done: true, summary: e.result || '', cost: total == null ? null : Math.max(total - spent, 0), duration: e.duration_ms ?? (turn.start != null && at != null ? at - turn.start : null)});
      if (total != null) spent = total;
    }
  }
  return turns;
}

export function place(turns, items) {
  const timed = turns.length > 0 && turns.every(t => t.start != null);
  if (!timed) return {placed: turns, unplaced: items};
  const placed = turns.map(t => ({...t, steps: [...t.steps]}));
  for (const item of items) placed[Math.max(placed.findLastIndex(t => t.start <= item.at), 0)].steps.push(item);
  for (const t of placed) t.steps.sort((a, b) => (a.at ?? 0) - (b.at ?? 0));
  return {placed, unplaced: []};
}

function fold(steps) {
  const out = [];
  for (const s of steps) {
    const last = out.at(-1);
    if (s.kind === 'reject' && last?.kind === 'reject' && last.method === s.method && last.path === s.path && last.error === s.error) last.count += 1;
    else if (s.kind === 'action' && last?.kind === 'action' && last.type === s.type) last.tasks.push(s.task);
    else out.push(s.kind === 'reject' ? {...s, count: 1} : s.kind === 'action' ? {...s, tasks: [s.task]} : s);
  }
  return out;
}

function Step({step, select}) {
  const [open, setOpen] = useState(false);
  if (step.kind === 'action') return <div className="pl-step k-action"><span className="k">{step.type === 'created' ? '+' : '✓'}</span><span className="what">{ACTION_WORDS[step.type] || step.type} {plural(step.tasks.length, 'task')}</span>
    <span className="why2">{step.tasks.map(t => <button key={t.task} className="link" title={t.title} onClick={() => select(t.task)}>{t.shortId || t.title}</button>)}</span></div>;
  if (step.kind === 'reject') return <div className="pl-step k-reject"><span className="k">✕</span><span className="what">{step.method} {step.path}</span><span className="why2">{step.error}</span>{step.count > 1 && <span className="x">×{step.count}</span>}</div>;
  if (step.kind === 'tool') return <div className={`pl-step k-tool${step.failed ? ' failed' : ''}`}><span className="k">›</span>
    <button className="what" aria-expanded={open} onClick={() => setOpen(!open)} title={step.full}>{step.text}</button>
    {open && <pre className="pl-output">{step.full}{step.output != null && `\n\n${step.output}`}</pre>}</div>;
  return <div className={`pl-step k-${step.kind}`}><span className="k">{step.kind === 'msg' ? '✎' : '…'}</span><span className="what">{step.text}</span></div>;
}

function Summary({text}) {
  const [full, setFull] = useState(false);
  const long = text.split('\n').length > 4 || text.length > 400;
  return <div className={`pl-summary${long && !full ? ' clamped' : ''}`}><span className="l">End of turn summary</span><div className="body">{text}</div>
    {long && <button className="link" onClick={() => setFull(!full)}>{full ? 'Show less' : 'Show all'}</button>}</div>;
}

function Turn({turn, number, open, toggle, select, running}) {
  const steps = fold(turn.steps);
  const facts = turn.done ? [clock(turn.start), turn.duration != null && duration(turn.duration), plural(turn.total ?? turn.steps.length, 'step'), turn.cost != null && money(turn.cost)] : [running ? 'running' : 'ended without a summary', plural(turn.total ?? turn.steps.length, 'step') + ' so far'];
  return <div className={`pl-turn${open ? '' : ' closed'}`}>
    <button className="th" aria-expanded={open} onClick={toggle}><span className="n">{open ? '▾' : '▸'} Turn {number}</span><span className="why">{turn.reason}</span><span className="r">{facts.filter(Boolean).join(' · ')}</span></button>
    {open && steps.map((s, i) => <Step key={i} step={s} select={select} />)}
    {open && !steps.length && <p className="pl-none">No steps match the filters.</p>}
    {turn.summary && <Summary text={turn.summary} />}
  </div>;
}

function useTranscript(session, number) {
  const [data, setData] = useState({lines: [], finished: false});
  const request = session?.requests[number - 1];
  useEffect(() => {
    if (!session || !request) return undefined;
    let live = true, timer, next = 0, lines = [];
    setData({lines: [], finished: false});
    const pull = async () => {
      try {
        const response = await fetch(`/api/sessions/${session.id}/requests/${number}/events?after=${next}`, {headers: {'X-Coordinator-Actor': 'ui'}});
        const value = await response.json();
        if (!live) return;
        if (response.ok) {
          next = value.next;
          if (value.lines.length) lines = [...lines, ...value.lines];
          setData({lines, finished: value.finished, error: ''});
          if (value.finished) return;
        } else setData({lines, finished: false, error: value.error});
      } catch (e) { if (live) setData({lines, finished: false, error: e.message}); }
      if (live) timer = setTimeout(pull, 1500);
    };
    pull();
    return () => { live = false; clearTimeout(timer); };
  }, [session?.id, number, !!request]);
  return data;
}

function FollowUp({session, api, close}) {
  const [text, setText] = useState('');
  const [error, setError] = useState('');
  return <form className="pl-follow" onSubmit={async e => { e.preventDefault(); try { await api(`/api/sessions/${session.id}/messages`, 'POST', {request: text}); close(); } catch (err) { setError(err.message); } }}>
    <label>Follow-up request<textarea rows="4" value={text} onChange={e => setText(e.target.value)} autoFocus /></label>
    {error && <p className="error">{error}</p>}
    <div className="pl-row"><button type="button" onClick={close}>Cancel</button><button className="primary" disabled={!text.trim()}>Send</button></div>
  </form>;
}

export default function Planner({workspace, tasks, select, api, names}) {
  const [sessions, setSessions] = useState(null);
  const [error, setError] = useState('');
  const [chosen, setChosen] = useState(null);
  const [picked, setPicked] = useState(null);
  const [shown, setShown] = useState(() => Object.fromEntries(FILTERS.map(([k, , on]) => [k, on])));
  const [follow, setFollow] = useState(true);
  const [opened, setOpened] = useState({});
  const [dialog, setDialog] = useState(null);
  const end = useRef(null);
  useEffect(() => {
    let live = true, timer;
    const poll = async () => {
      try { const list = await api('/api/sessions'); if (live) { setSessions(list); setError(''); } } catch (e) { if (live) setError(e.message); }
      if (live) timer = setTimeout(poll, 1500);
    };
    poll();
    return () => { live = false; clearTimeout(timer); };
  }, [api]);
  const now = Date.now();
  const scoped = useMemo(() => (sessions || []).filter(s => !workspace || within(s.workspace, workspace) || within(workspace, s.workspace)).sort((a, b) => b.created - a.created), [sessions, workspace]);
  const session = scoped.find(s => s.id === chosen) || scoped[0];
  const number = session ? Math.min(picked?.session === session.id ? picked.number : session.requests.length, session.requests.length) : 0;
  const request = session?.requests[number - 1];
  const transcript = useTranscript(session, number);
  const firstReason = request ? request.kind === 'wake' ? `Resumed by the coordinator: ${firstLine(request.text)}` : `Started by the ${(KIND_WORDS[request.kind] || 'initial').toLowerCase()} request` : '';
  const turns = useMemo(() => parseTurns(transcript.lines, firstReason), [transcript.lines, firstReason]);
  const rejections = (request?.rejected || []).map(r => ({kind: 'reject', ...r}));
  const actions = (session?.actions || []).filter(a => request && a.at >= request.started && (!session.requests[number] || a.at < session.requests[number].started)).map(a => ({kind: 'action', type: a.kind, at: a.at, task: a}));
  const {placed, unplaced} = place(turns, [...actions, ...rejections]);
  const running = request && !request.planner_ended;
  const lastTurn = turns.length - 1;
  const isOpen = i => opened[`${session?.id}:${number}:${i}`] ?? (running && i === lastTurn);
  useEffect(() => { if (follow && running) end.current?.scrollIntoView({block: 'nearest'}); }, [transcript.lines.length, follow, running]);

  if (sessions === null) return <p role="status">{error || 'Loading sessions…'}</p>;
  if (!scoped.length) return <p className="empty">No planner sessions in this workspace yet.</p>;

  const cost = s => s.requests.reduce((sum, r) => sum + (r.usage?.costUsd || 0), 0) || null;
  const rejectedCount = s => s.requests.reduce((sum, r) => sum + (r.rejected?.length || 0), 0);
  const elapsed = s => (s.requests.at(-1)?.ended || (s.status === 'running' ? now : s.requests.at(-1)?.planner_ended) || now) - (s.requests[0]?.started || s.created);
  const shownCost = (cost(session) || 0) + (running ? turns.reduce((sum, t) => sum + (t.cost || 0), 0) : 0);

  const effects = Object.entries(ACTION_WORDS).map(([kind, label]) => [label, (session.actions || []).filter(a => a.kind === kind)]);
  const grouped = Object.entries(session.requests.flatMap(r => r.rejected || []).reduce((m, r) => ({...m, [r.error]: (m[r.error] || 0) + 1}), {})).sort((a, b) => b[1] - a[1]);
  const waiting = tasks.filter(t => within(t.workspace ?? t.cwd, session.workspace) && !t.container && (t.status === 'In progress' || t.status === 'Review'));
  const toggleFilter = k => setShown({...shown, [k]: !shown[k]});
  const visible = placed.map(t => ({...t, total: t.steps.length, steps: t.steps.filter(s => shown[s.kind])}));
  const label = s => <>{names?.[Object.keys(names).find(k => pathKey(k) === pathKey(s.root))] && <span className="task-id">{names[Object.keys(names).find(k => pathKey(k) === pathKey(s.root))]}</span>}{folder(s.workspace)}</>;

  return <div className="pl page-enter">
    <div className="pl-sessions" aria-label="Sessions">
      <h4><span>Sessions</span><span>newest first</span></h4>
      {scoped.map(s => <button key={s.id} className={`pl-srow ${s.status}${s === session ? ' on' : ''}`} aria-current={s === session ? 'true' : undefined} onClick={() => setChosen(s.id)}>
        <span className="t"><span>{label(s)}</span><span className={`pill ${STATUS_TONE[s.status] ?? ''}`}>{s.status}</span></span>
        <span className="m">{[plural(s.requests.length, 'request'), duration(elapsed(s)), money(cost(s)), `${rejectedCount(s)} rejected`].join(' · ')}</span>
      </button>)}
    </div>
    <div className="pl-detail">
      <div className="pl-head">
        <span className="name">{label(session)}</span>
        <span className={`pill ${STATUS_TONE[session.status] ?? ''}`}>{session.status}{running ? ` · turn ${turns.length || 1}` : ''}</span>
        {session.agent && <span className="muted">{session.agent.model}{session.agent.effort ? ` · ${session.agent.effort}` : ''}</span>}
        <span className="spacer" />
        <button onClick={() => setDialog('request')}>Open request</button>
        <button disabled={session.status !== 'waiting'} onClick={() => setDialog('follow')}>Send follow-up</button>
        <button className="danger" disabled={session.status !== 'running'} onClick={() => window.confirm('Stop this planner session and its tasks?') && api(`/api/sessions/${session.id}/stop`, 'POST', {}).catch(e => setError(e.message))}>Stop session</button>
      </div>
      {error && <p className="error">{error}</p>}
      {session.wake_error && <p className="error" role="alert">The coordinator could not wake the planner: {session.wake_error}</p>}
      {dialog === 'request' && <div className="pl-box pl-dialog"><div className="pl-row"><h4>Requests</h4><span className="spacer" /><button onClick={() => setDialog(null)}>Close</button></div>
        {session.requests.map((r, i) => <div key={i}><b>{i + 1} · {KIND_WORDS[r.kind] || 'Initial'}</b> <span className="muted">{new Date(r.started).toLocaleString()}</span><pre className="pl-output">{r.text}</pre></div>)}</div>}
      {dialog === 'follow' && <FollowUp session={session} api={api} close={() => setDialog(null)} />}
      <div className="pl-metrics">
        {[['Elapsed', duration(elapsed(session))], ['Turns', turns.length], ['Cost so far', money(shownCost || null)], ['Board actions', (session.actions || []).length]].map(([l, v]) => <div key={l} className="pl-metric"><div className="l">{l}</div><div className="v">{v}</div></div>)}
        <div className={`pl-metric${rejectedCount(session) ? ' bad' : ''}`}><div className="l">Rejected calls</div><div className="v">{rejectedCount(session)}</div></div>
      </div>
      <div className="pl-requests" aria-label="Requests"><span className="muted">Requests</span>
        {session.requests.map((r, i) => <button key={i} className={`pl-req${i + 1 === number ? ' on' : ''}`} aria-pressed={i + 1 === number} onClick={() => setPicked({session: session.id, number: i + 1})} title={r.text}>
          <b>{i + 1} · {KIND_WORDS[r.kind] || 'Initial'}</b> · {clock(r.started)} · {r.planner_ended ? (r.exit_code === 0 ? 'ended' : `exit ${r.exit_code ?? '?'}`) : 'running'}</button>)}
      </div>
      {request && request.planner_ended && (request.exit_code !== 0 || request.error) && <p className="error">Planner exited with code {request.exit_code ?? 'unknown'}{request.error ? `: ${request.error}` : ''}</p>}
      <div className="pl-filters"><span className="muted">Show</span>
        {FILTERS.map(([k, l]) => <button key={k} className="pl-chip" aria-pressed={shown[k]} onClick={() => toggleFilter(k)}>{l}</button>)}
        <span className="spacer" />
        <button className="pl-chip" aria-pressed={follow} onClick={() => setFollow(!follow)}>Follow live</button>
      </div>
      <div className="pl-cols">
        <div className="pl-timeline">
          {transcript.error && <p className="error pl-none">{transcript.error}</p>}
          {!turns.length && <p className="pl-none">{running ? 'Waiting for the planner’s first output…' : 'This request has no transcript.'}</p>}
          {visible.map((t, i) => <Turn key={i} turn={t} number={i + 1} running={running && i === lastTurn} open={isOpen(i)} select={select} toggle={() => setOpened({...opened, [`${session.id}:${number}:${i}`]: !isOpen(i)})} />)}
          {!!unplaced.length && <div className="pl-turn"><div className="th"><span className="n">Not placed in turns</span><span className="why">This transcript has no arrival times</span></div>{fold(unplaced.filter(s => shown[s.kind])).map((s, i) => <Step key={i} step={s} select={select} />)}</div>}
          <div ref={end} />
        </div>
        <div className="pl-side">
          <div className="pl-box"><h4>Board effects</h4>
            {effects.map(([label, list]) => <details key={label} className="pl-kv"><summary><span>{label}</span><span>{new Set(list.map(a => a.task)).size}</span></summary>
              {[...new Map(list.map(a => [a.task, a])).values()].map(a => <button key={a.task} className="link" onClick={() => select(a.task)}>{a.shortId ? `${a.shortId} ` : ''}{a.title}</button>)}</details>)}
          </div>
          <div className="pl-box"><h4>Most rejected</h4>{grouped.length ? <ul>{grouped.map(([e, n]) => <li key={e}>{e} <span className="muted">×{n}</span></li>)}</ul> : <p className="muted">None</p>}</div>
          <div className="pl-box"><h4>Being worked on</h4>{waiting.length ? <ul>{waiting.map(t => <li key={t.id}><button className="link" onClick={() => select(t.id)}>{t.shortId ? `${t.shortId} ` : ''}{t.title}</button> <span className="muted">{t.status}</span></li>)}</ul> : <p className="muted">Nothing</p>}</div>
        </div>
      </div>
    </div>
  </div>;
}
