import React, {useEffect, useRef, useState} from 'react';
import {Archive, ArrowRight, CheckCircle2, ChevronRight, CircleDashed, Clock, Coins, ExternalLink, GitBranch, Layers, Link2, ListChecks, Pause, Pencil, Play, RefreshCw, RotateCcw, ScrollText, Timer, X, XCircle} from 'lucide-react';
import {BorderBeam} from './Activity';
import TaskName from './TaskName';
import {attemptMs, categoryName, compact, duration, failed, leadMs, phase, tokens, workMs, waitMs} from './fields';

const ROLE_HUE = {worker: 220, analytics: 190, 'backend-code': 222, 'ui-code': 280, design: 320, optimization: 30, test: 140, docs: 40, data: 160, audit: 12};
const roleStyle = role => ({'--tag-hue': ROLE_HUE[role] ?? 220});
const target = d => typeof d === 'string' ? d : d.target;
const ago = ms => {
  if (!ms) return '';
  const s = Math.round((Date.now() - ms) / 1000);
  return s < 60 ? 'just now' : s < 3600 ? `${Math.floor(s / 60)}m ago` : s < 86400 ? `${Math.floor(s / 3600)}h ago` : new Date(ms).toLocaleDateString();
};

function useTicker(active) {
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!active) return;
    const id = setInterval(() => setTick(n => n + 1), 1000);
    return () => clearInterval(id);
  }, [active]);
}

export function TaskCard({task: t, paused, offline, open}) {
  const p = phase(t, paused);
  const work = t.attempts?.length ? workMs(t) : null;
  const tags = [...(t.attributes.languages || []).map(v => ['lang', v]), ...(t.attributes.functions || []).map(v => ['fn', v])];
  return <article className={`card tone-${p.tone || 'idle'} ${t.stopped ? 'is-stopped' : ''}`} role="button" tabIndex={0} aria-label={`${t.shortId || ''} ${t.title}. ${t.status}. ${p.text}`}
    onClick={() => open(t.id)} onKeyDown={e => {if (e.key === 'Enter' || e.key === ' ') {e.preventDefault(); open(t.id);}}}>
    {p.tone === 'live' && !offline && <BorderBeam />}
    <header className="card-top">
      {t.shortId && <span className="task-id">{t.shortId}</span>}
      <span className={`prio p${t.attributes.priority ?? 2}`} title="Priority">P{t.attributes.priority ?? 2}</span>
      {t.container && <span className="mini-badge" title="Container"><Layers size={12} aria-hidden="true" />{t.children.length}</span>}
      {t.parent && <span className="mini-badge" title="Child task"><GitBranch size={12} aria-hidden="true" />child</span>}
      <span className="spacer" />
      {!!t.attempts?.length && <span className="mini-stat" title="Attempts"><RotateCcw size={12} aria-hidden="true" />{t.attempts.length}</span>}
    </header>
    <h3 className="card-title">{t.title}</h3>
    <div className="card-tags">
      <span className="tag role" style={roleStyle(t.attributes.role)}>{t.attributes.role}</span>
      {tags.slice(0, 4).map(([kind, v]) => <span key={kind + v} className={`tag ${kind}`}>{categoryName(v)}</span>)}
      {tags.length > 4 && <span className="tag more">+{tags.length - 4}</span>}
    </div>
    <footer className="card-foot">
      <span className={`status-dot ${p.tone}`} aria-hidden="true" /><span className={`card-phase ${p.tone}`}>{p.text}</span>
      <span className="spacer" />
      {work != null && <span className="mini-stat" title="Working time"><Clock size={12} aria-hidden="true" />{duration(work)}</span>}
    </footer>
  </article>;
}

function Section({icon: Icon, title, count, children, tone}) {
  return <section className={`detail-section ${tone || ''}`}>
    <h3><Icon size={15} aria-hidden="true" />{title}{count != null && <span className="count">{count}</span>}</h3>
    {children}
  </section>;
}

function RelationLink({id, task, reason, select}) {
  if (!task) return <span className="relation missing">Unavailable task <code>{id}</code></span>;
  const p = phase(task, false);
  return <button type="button" className="relation" onClick={() => select(id)}>
    <span className={`status-dot ${p.tone}`} aria-hidden="true" />
    <span className="relation-name"><TaskName task={task} /></span>
    {reason && <span className="relation-reason">{reason.replaceAll('_', ' ')}</span>}
    <span className="relation-status">{task.status}</span>
    <ChevronRight size={15} aria-hidden="true" className="relation-go" />
  </button>;
}

function Attempt({task, attempt: a, index}) {
  const running = task.active === a.id && !a.ended;
  const bad = failed(a);
  const status = running ? 'running' : a.error ? 'error' : a.result?.status || (a.ended ? 'ended' : 'running');
  const usage = a.evidence?.usage?.totalTokens;
  const model = a.evidence?.observed?.model;
  return <li className={`attempt ${running ? 'live' : bad ? 'bad' : 'good'}`}>
    <span className="attempt-node" aria-hidden="true">{running ? <CircleDashed size={14} /> : bad ? <XCircle size={14} /> : <CheckCircle2 size={14} />}</span>
    <div className="attempt-body">
      <div className="attempt-head">
        <strong>#{index} {categoryName(a.stage)}</strong>
        <span className={`badge ${running ? 'live' : bad ? 'bad' : 'good'}`}>{status}</span>
        <span className="muted">{ago(a.started)}</span>
        <span className="spacer" />
        <span className="mini-stat" title="Duration"><Timer size={12} aria-hidden="true" />{duration(attemptMs(task, a))}</span>
        {usage ? <span className="mini-stat" title="Tokens"><Coins size={12} aria-hidden="true" />{compact(usage)}</span> : null}
        <a className="icon-link" href={`/api/attempts/${a.id}/log`} target="_blank" rel="noreferrer" title="Open log"><ScrollText size={14} aria-hidden="true" /><span className="sr-only">Open log</span></a>
      </div>
      {model && <div className="muted attempt-model">{model}{a.evidence?.observed?.forwardedEffort ? ` · ${a.evidence.observed.forwardedEffort}` : ''}</div>}
      {a.error && <p className="attempt-error">{a.error}</p>}
      {a.result?.summary && <p className="attempt-summary">{a.result.summary}</p>}
      {!!a.result?.findings?.length && <details className="attempt-findings"><summary>{a.result.findings.length} finding{a.result.findings.length > 1 ? 's' : ''}</summary><ul>{a.result.findings.map((f, i) => <li key={i}>{f}</li>)}</ul></details>}
    </div>
  </li>;
}

export function Detail({task: t, byId, paused, wordLimit, reasons, close, select, act, edit}) {
  const ref = useRef(null);
  useTicker(!!t.active);
  useEffect(() => {
    const opener = document.activeElement, dialog = ref.current;
    dialog.showModal();
    return () => {dialog.close(); if (opener?.isConnected) opener.focus();};
  }, []);
  useEffect(() => {ref.current?.querySelector('.drawer-scroll')?.scrollTo({top: 0});}, [t.id]);
  const p = phase(t, paused);
  const all = Object.values(byId);
  const latest = [...t.attempts].reverse().find(a => a.revision === t.revision && a.result?.criteria);
  const blocks = all.flatMap(task => task.dependencies.filter(d => target(d) === t.id).map(d => ({id: task.id, reason: d.reason})));
  const children = [...new Set([...t.children, ...all.filter(task => task.parent === t.id).map(task => task.id)])];
  const groups = [
    ['Needs results from', t.dependencies.map(d => ({id: target(d), reason: typeof d === 'object' && reasons.includes(d.reason) ? d.reason : typeof d === 'object' ? `unresolved ${d.reason || ''}` : ''}))],
    ['Required by', blocks],
    ['Runs after', t.after.map(id => ({id}))],
    ['Parent', t.parent ? [{id: t.parent}] : []],
    ['Children', children.map(id => ({id}))],
    ['Never runs alongside', t.conflicts.map(id => ({id}))],
    ['Related', t.related.map(id => ({id}))],
  ].filter(([, list]) => list.length);
  const control = t.stopped || t.blocked ? 'resume' : 'stop';
  const work = t.attempts.length ? workMs(t) : null, lead = leadMs(t), wait = waitMs(t), used = tokens(t);
  const failures = t.attempts.filter(failed).length;
  const properties = [
    ['Role', <span className="tag role" style={roleStyle(t.attributes.role)}>{t.attributes.role}</span>],
    ['Priority', <span className={`prio p${t.attributes.priority ?? 2}`}>P{t.attributes.priority ?? 2}</span>],
    ['Size', categoryName(t.attributes.size || 'small')],
    ['Artifact', `${categoryName(t.attributes.artifact || 'none')}${t.attributes.deploy ? ' · deploys' : ''}`],
    ['Languages', t.attributes.languages?.length ? <span className="tag-list">{t.attributes.languages.map(v => <span key={v} className="tag lang">{categoryName(v)}</span>)}</span> : <span className="muted">Uncategorized</span>],
    ['Functions', t.attributes.functions?.length ? <span className="tag-list">{t.attributes.functions.map(v => <span key={v} className="tag fn">{categoryName(v)}</span>)}</span> : <span className="muted">Uncategorized</span>],
    ['Capabilities', t.attributes.needs?.length ? t.attributes.needs.join(', ') : <span className="muted">None</span>],
    ['Models', t.models?.length ? t.models.map(m => m.effort ? `${m.model} · ${m.effort}` : m.model).join(', ') : <span className="muted">Not run yet</span>],
    ['Grouping', t.container ? 'Container' : t.parent ? 'Child task' : t.standalone ? 'Standalone' : 'Unresolved'],
  ];
  return <dialog ref={ref} className="drawer" onCancel={e => {e.preventDefault(); close();}} onClick={e => {if (e.target === ref.current) close();}} aria-labelledby="drawer-title">
    <div className={`drawer-inner tone-${p.tone || 'idle'}`} key={t.id}>
      <header className="drawer-head">
        <div className="drawer-kicker">
          {t.shortId && <span className="task-id">{t.shortId}</span>}
          <span className={`badge status-${t.status.replace(/\s/g, '').toLowerCase()}`}>{t.status}</span>
          <span className="muted">{categoryName(t.stage)} · rev {t.revision}</span>
          <span className="spacer" />
          <button type="button" className="icon-button" onClick={close} aria-label="Close"><X size={18} aria-hidden="true" /></button>
        </div>
        <h2 id="drawer-title">{t.title}</h2>
        <p className={`drawer-phase ${p.tone}`}><span className={`status-dot ${p.tone}`} aria-hidden="true" />{p.text}</p>
        <div className="drawer-actions">
          {t.stage !== 'done' && <button type="button" className="tool" onClick={() => act(`/api/tasks/${t.id}/${control}`)}>{control === 'resume' ? <Play size={14} aria-hidden="true" /> : <Pause size={14} aria-hidden="true" />}{control === 'resume' ? 'Resume' : 'Stop'}</button>}
          {t.stage !== 'done' && !t.container && <button type="button" className="tool" disabled={!!t.active} onClick={() => act(`/api/tasks/${t.id}/replan`, 'POST')}><RefreshCw size={14} aria-hidden="true" />Replan</button>}
          <button type="button" className="tool" disabled={!!t.active} onClick={() => edit(t)}><Pencil size={14} aria-hidden="true" />Edit</button>
          <span className="spacer" />
          <button type="button" className="tool danger" disabled={!!t.active} onClick={async () => {if (confirm('Archive this task?') && await act(`/api/tasks/${t.id}`, 'DELETE')) close();}}><Archive size={14} aria-hidden="true" />Archive</button>
        </div>
      </header>
      <div className="drawer-scroll">
        <div className="stat-strip">
          <div className="stat"><span>Working</span><strong>{duration(work)}</strong></div>
          <div className="stat"><span>Lead time</span><strong>{duration(lead)}</strong></div>
          <div className="stat"><span>Waiting</span><strong>{duration(wait)}</strong></div>
          <div className={`stat ${failures ? 'bad' : ''}`}><span>Attempts</span><strong>{t.attempts.length}{failures ? <small> · {failures} failed</small> : null}</strong></div>
          <div className="stat"><span>Tokens</span><strong>{compact(used || null)}</strong></div>
          <div className={`stat ${t.definitionWords > wordLimit ? 'bad' : ''}`}><span>Words</span><strong>{t.definitionWords}<small> / {wordLimit}</small></strong></div>
        </div>
        <Section icon={ArrowRight} title="Outcome"><p className="outcome">{t.outcome}</p></Section>
        {!!t.acceptance.length && <Section icon={ListChecks} title="Acceptance" count={latest ? `${latest.result.criteria.filter(c => c === 'passed').length}/${t.acceptance.length} passed` : t.acceptance.length}>
          <ol className="criteria">{t.acceptance.map((c, i) => {
            const outcome = latest?.result.criteria[i];
            return <li key={i} className={outcome === 'passed' ? 'good' : outcome ? 'bad' : ''}><span className="criterion-mark" aria-label={outcome || 'not checked'}>{outcome === 'passed' ? <CheckCircle2 size={15} /> : outcome ? <XCircle size={15} /> : <CircleDashed size={15} />}</span><span>{c}</span></li>;
          })}</ol>
        </Section>}
        {!!t.feedback.length && <Section icon={RotateCcw} title="Feedback for next attempt" tone="callout"><ul>{t.feedback.map((f, i) => <li key={i}>{f}</li>)}</ul></Section>}
        <Section icon={Layers} title="Properties"><dl className="props">{properties.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl></Section>
        {!!groups.length && <Section icon={Link2} title="Relationships">{groups.map(([label, list]) => <div className="relation-group" key={label}><h4>{label}<span className="count">{list.length}</span></h4>
          {list.map(({id, reason}) => <RelationLink key={id + (reason || '')} id={id} task={byId[id]} reason={reason} select={select} />)}</div>)}</Section>}
        <Section icon={Clock} title="Attempts" count={t.attempts.length}>
          {t.attempts.length ? <ol className="timeline">{[...t.attempts].map((a, i) => [a, i + 1]).reverse().map(([a, n]) => <Attempt key={a.id} task={t} attempt={a} index={n} />)}</ol> : <p className="muted">No attempts yet.</p>}
        </Section>
        <footer className="drawer-foot"><ExternalLink size={13} aria-hidden="true" /><span className="path">{(t.workspace || t.cwd || '').replace(/^\\\\\?\\/, '')}</span></footer>
      </div>
    </div>
  </dialog>;
}
