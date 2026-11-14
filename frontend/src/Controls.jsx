import React, {useEffect, useMemo, useRef, useState} from 'react';
import {ChevronDown, Filter, FolderTree, Search, SlidersHorizontal, X, Check, Palette} from 'lucide-react';
import {AnimatedCount} from './Activity';
import {FIELDS, FIELD, GROUPS} from './fields';

export function Popover({label, icon: Icon, badge, children, align = 'left', className = ''}) {
  const [open, setOpen] = useState(false);
  const root = useRef(null);
  useEffect(() => {
    if (!open) return;
    const outside = event => {if (!root.current?.contains(event.target)) setOpen(false);};
    const escape = event => {if (event.key === 'Escape') {setOpen(false); root.current?.querySelector('button')?.focus();}};
    document.addEventListener('pointerdown', outside);
    document.addEventListener('keydown', escape);
    return () => {document.removeEventListener('pointerdown', outside); document.removeEventListener('keydown', escape);};
  }, [open]);
  return <div className={`popover ${className}`} ref={root}>
    <button type="button" className={`tool ${open ? 'on' : ''}`} aria-expanded={open} aria-haspopup="dialog" onClick={() => setOpen(o => !o)}>
      {Icon && <Icon size={15} aria-hidden="true" />}<span>{label}</span>{badge ? <span className="tool-badge">{badge}</span> : null}<ChevronDown size={14} aria-hidden="true" className="caret" />
    </button>
    {open && <div className={`popover-panel ${align}`} role="dialog" aria-label={label}>{typeof children === 'function' ? children(() => setOpen(false)) : children}</div>}
  </div>;
}

function AddFilter({tasks, filters, add}) {
  const [field, setField] = useState(null);
  const [needle, setNeedle] = useState('');
  const counts = useMemo(() => {
    if (!field) return [];
    const tally = new Map();
    tasks.forEach(t => FIELD[field].values(t).forEach(v => tally.set(v, (tally.get(v) || 0) + 1)));
    return [...tally].sort((a, b) => b[1] - a[1] || String(a[0]).localeCompare(String(b[0])));
  }, [field, tasks]);
  const categories = FIELDS.filter(f => f.type === 'category' && f.label.toLowerCase().includes(needle.toLowerCase()));
  return <Popover label="Filter" icon={Filter} badge={filters.length || null}>
    {close => !field ? <div className="picker">
      <label className="picker-search"><Search size={14} aria-hidden="true" /><input autoFocus placeholder="Filter by field…" aria-label="Search filter fields" value={needle} onChange={e => setNeedle(e.target.value)} /></label>
      <ul className="picker-list">{categories.map(f => <li key={f.key}><button type="button" className="picker-item" onClick={() => {setField(f.key); setNeedle('');}}><span>{f.label}</span><small>{f.group}</small></button></li>)}</ul>
    </div> : <div className="picker">
      <div className="picker-head"><button type="button" className="link" onClick={() => setField(null)}>← Fields</button><strong>{FIELD[field].label}</strong></div>
      <ul className="picker-list">{counts.map(([value, count]) => {
        const on = filters.some(f => f.field === field && f.value === value);
        return <li key={value}><button type="button" className={`picker-item ${on ? 'on' : ''}`} aria-pressed={on} onClick={() => {add(field, value); if (!on) close();}}>
          <span className="check-mark">{on && <Check size={13} aria-hidden="true" />}</span><span>{value}</span><small>{count}</small></button></li>;
      })}{!counts.length && <li className="muted picker-empty">No values in this scope</li>}</ul>
    </div>}
  </Popover>;
}

export function ScopeBar({workspaces, workspace, setWorkspace, query, setQuery, filters, setFilters, tasks, count, total, showFilters = true}) {
  const toggle = (field, value) => setFilters(list => list.some(f => f.field === field && f.value === value) ? list.filter(f => !(f.field === field && f.value === value)) : [...list, {field, value}]);
  const short = path => path.split(/[\\/]/).filter(Boolean).slice(-2).join('/');
  return <div className="scopebar" role="search">
    <label className="scope-workspace" title={workspace || 'All workspaces'}><FolderTree size={15} aria-hidden="true" />
      <select aria-label="Workspace" value={workspaces.find(path => path.toLowerCase() === workspace.toLowerCase()) || ''} onChange={e => setWorkspace(e.target.value)}>
        <option value="">All workspaces</option>{workspaces.map(w => <option key={w} value={w}>{short(w)}</option>)}
      </select>
    </label>
    {showFilters && <>
      <label className="scope-search"><Search size={15} aria-hidden="true" /><input type="search" aria-label="Search tasks by ID or title" placeholder="Search ID or title" value={query} onChange={e => setQuery(e.target.value)} /></label>
      <AddFilter tasks={tasks} filters={filters} add={toggle} />
      <div className="chips-row" aria-label="Active filters">{filters.map(f => <span className="filter-chip" key={`${f.field}:${f.value}`}><span className="filter-chip-field">{FIELD[f.field]?.label}</span>{f.value}
        <button type="button" aria-label={`Remove ${FIELD[f.field]?.label} ${f.value} filter`} onClick={() => toggle(f.field, f.value)}><X size={12} aria-hidden="true" /></button></span>)}</div>
      {(filters.length > 0 || query) && <button type="button" className="tool ghost" onClick={() => {setFilters([]); setQuery('');}}>Clear</button>}
      <span className="scope-count"><AnimatedCount value={count} />{count !== total && <span className="muted"> of {total}</span>} tasks</span>
    </>}
  </div>;
}

export function FieldPicker({fields, setFields, colorBy, setColorBy}) {
  const [needle, setNeedle] = useState('');
  const shown = FIELDS.filter(f => f.label.toLowerCase().includes(needle.toLowerCase()));
  const toggle = key => setFields(list => list.includes(key) ? list.filter(k => k !== key) : [...list, key]);
  return <>
    <Popover label="Fields" icon={SlidersHorizontal} badge={fields.length || null}>
      <div className="picker wide">
        <label className="picker-search"><Search size={14} aria-hidden="true" /><input autoFocus placeholder="Find a field…" aria-label="Search fields" value={needle} onChange={e => setNeedle(e.target.value)} /></label>
        {!!fields.length && <div className="picker-selected">{fields.map(key => <span className="filter-chip" key={key}>{FIELD[key]?.label}<button type="button" aria-label={`Hide ${FIELD[key]?.label}`} onClick={() => toggle(key)}><X size={12} aria-hidden="true" /></button></span>)}</div>}
        <div className="picker-groups">{GROUPS.map(group => {
          const items = shown.filter(f => f.group === group);
          return items.length ? <section key={group}><h4>{group}</h4><ul className="picker-list">{items.map(f => {
            const on = fields.includes(f.key);
            return <li key={f.key}><button type="button" className={`picker-item ${on ? 'on' : ''}`} aria-pressed={on} onClick={() => toggle(f.key)}><span className="check-mark">{on && <Check size={13} aria-hidden="true" />}</span><span>{f.label}</span></button></li>;
          })}</ul></section> : null;
        })}</div>
      </div>
    </Popover>
    <label className="tool select-tool"><Palette size={15} aria-hidden="true" /><span className="sr-only">Color nodes by</span>
      <select aria-label="Color nodes by" value={colorBy} onChange={e => setColorBy(e.target.value)}>
        <option value="">Color: status</option>
        {GROUPS.map(group => <optgroup key={group} label={group}>{FIELDS.filter(f => f.group === group).map(f => <option key={f.key} value={f.key}>Color: {f.label}</option>)}</optgroup>)}
      </select>
    </label>
  </>;
}

export function usePreference(key, fallback) {
  const [value, setValue] = useState(() => {try {const saved = JSON.parse(localStorage.getItem(key)); return saved ?? fallback;} catch {return fallback;}});
  const set = next => setValue(previous => {
    const resolved = typeof next === 'function' ? next(previous) : next;
    try {localStorage.setItem(key, JSON.stringify(resolved));} catch {}
    return resolved;
  });
  return [value, set];
}

