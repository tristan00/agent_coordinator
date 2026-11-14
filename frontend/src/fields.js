const sum = list => list.reduce((a, b) => a + b, 0);
const target = d => typeof d === 'string' ? d : d.target;

export const categoryName = value => ({cpp: 'C++', csharp: 'C#', javascript: 'JavaScript', typescript: 'TypeScript', powershell: 'PowerShell', api: 'API', ui: 'UI', css: 'CSS', html: 'HTML', sql: 'SQL'}[value] || String(value).charAt(0).toUpperCase() + String(value).slice(1));

export function duration(ms) {
  if (ms == null || !Number.isFinite(ms)) return '—';
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, '0')}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, '0')}m`;
}

export const compact = n => n == null || !Number.isFinite(n) ? '—' : n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${(n / 1e3).toFixed(n >= 1e4 ? 0 : 1)}k` : String(n);

export const attemptMs = (t, a, now = Date.now()) => (a.ended ?? (t.active === a.id ? now : a.started)) - a.started;
export const workMs = (t, now) => sum((t.attempts || []).map(a => attemptMs(t, a, now))) || null;
export const leadMs = (t, now = Date.now()) => t.analytics?.leadMs ?? (t.analytics?.createdAt ? now - t.analytics.createdAt : null);
export const waitMs = t => t.analytics?.intervals?.milliseconds ? sum(Object.values(t.analytics.intervals.milliseconds)) : null;
export const tokens = t => sum((t.attempts || []).map(a => a.evidence?.usage?.totalTokens || 0));
export const failed = a => !!a.error || (a.ended && a.result && a.result.status !== 'completed');
export const workspaceName = t => (t.workspace || t.cwd || '').replace(/^\\\\\?\\/, '').split(/[\\/]/).filter(Boolean).slice(-2).join('/');

export function phase(t, paused) {
  if (t.blocked) return {text: `Needs decision: ${t.failure}`, tone: 'bad'};
  if (t.stage === 'done') return {text: t.container ? 'All child tasks done' : 'Accepted', tone: 'good'};
  if (t.stopped || paused) return {text: t.active ? 'Stopping' : 'Stopped', tone: 'muted'};
  if (t.active) return {text: t.stage === 'replan' ? 'Replanning' : t.stage === 'review' ? 'Reviewing' : 'Implementing', tone: 'live'};
  if (t.stage === 'replan') return {text: 'Queued for replanning', tone: 'warn'};
  return {text: t.waitingReason || (t.failure ? `Retrying: ${t.failure}` : 'Ready'), tone: t.waitingReason ? 'muted' : t.failure ? 'warn' : ''};
}

const category = (key, label, group, values) => ({key, label, group, type: 'category', values});
const number = (key, label, group, value, format = String) => ({key, label, group, type: 'number', value, format});

export const FIELDS = [
  category('status', 'Status', 'Workflow', t => [t.status]),
  category('control', 'Control', 'Workflow', t => [t.blocked ? 'Needs decision' : t.stopped ? 'Stopped' : t.active ? 'Running' : 'Idle']),
  category('stage', 'Stage', 'Workflow', t => [categoryName(t.stage)]),
  category('role', 'Role', 'Classification', t => [t.attributes.role]),
  category('languages', 'Languages', 'Classification', t => t.attributes.languages?.length ? t.attributes.languages.map(categoryName) : ['Uncategorized']),
  category('functions', 'Functions', 'Classification', t => t.attributes.functions?.length ? t.attributes.functions.map(categoryName) : ['Uncategorized']),
  category('needs', 'Capabilities', 'Classification', t => t.attributes.needs?.length ? t.attributes.needs : ['None']),
  category('priority', 'Priority', 'Classification', t => [`P${t.attributes.priority ?? 2}`]),
  category('size', 'Size', 'Classification', t => [categoryName(t.attributes.size || 'small')]),
  category('artifact', 'Artifact', 'Classification', t => [categoryName(t.attributes.artifact || 'none')]),
  category('model', 'Model', 'Execution', t => t.models?.length ? [...new Set(t.models.map(m => m.model))] : ['Not run yet']),
  category('grouping', 'Grouping', 'Structure', t => [t.container ? 'Container' : t.parent ? 'Child' : 'Standalone']),
  category('workspace', 'Workspace', 'Structure', t => [workspaceName(t)]),
  number('work', 'Working time', 'Execution', t => t.attempts?.length ? workMs(t) : null, duration),
  number('lead', 'Lead time', 'Execution', t => leadMs(t), duration),
  number('wait', 'Waiting time', 'Execution', waitMs, duration),
  number('attempts', 'Attempts', 'Execution', t => t.attempts?.length || 0),
  number('failures', 'Failed attempts', 'Execution', t => (t.attempts || []).filter(failed).length),
  number('tokens', 'Tokens', 'Execution', t => tokens(t) || null, compact),
  number('revision', 'Revision', 'Structure', t => t.revision),
  number('words', 'Definition words', 'Structure', t => t.definitionWords),
  number('criteria', 'Criteria', 'Structure', t => t.acceptance?.length || 0),
  number('dependencies', 'Dependencies', 'Structure', t => (t.dependencies || []).map(target).length),
  number('children', 'Children', 'Structure', t => t.children?.length || 0),
];
export const FIELD = Object.fromEntries(FIELDS.map(f => [f.key, f]));
export const GROUPS = [...new Set(FIELDS.map(f => f.group))];

export function display(field, t) {
  if (field.type === 'category') return field.values(t).join(', ');
  const value = field.value(t);
  return value == null ? '—' : field.format(value);
}

const PALETTE = ['#6e9bff', '#f2b45a', '#5ccf8d', '#f27b8a', '#b48cf2', '#4fc8e8', '#f59a5b', '#7fd4c1', '#d9d36a', '#e88ad6'];
const TONES = {'To do': '#8b95a7', 'In progress': '#4fa8ff', Review: '#f2b45a', Done: '#5ccf8d', Running: '#4fa8ff', Stopped: '#8b95a7', Idle: '#6e7a8f', 'Needs decision': '#f27b8a'};

export function colorScale(field, tasks) {
  if (!field) return null;
  if (field.type === 'category') {
    const values = [...new Set(tasks.flatMap(t => field.values(t)))].sort();
    const color = value => TONES[value] || PALETTE[values.indexOf(value) % PALETTE.length];
    return {color: t => color(field.values(t)[0]), legend: values.map(value => ({label: value, color: color(value)}))};
  }
  const numbers = tasks.map(t => field.value(t)).filter(Number.isFinite);
  const low = Math.min(...numbers), high = Math.max(...numbers);
  const at = x => `hsl(${Math.round(210 - 190 * x)} 80% 60%)`;
  const scaled = v => high > low ? (v - low) / (high - low) : 0.5;
  return {color: t => Number.isFinite(field.value(t)) ? at(scaled(field.value(t))) : '#6e7a8f',
    legend: numbers.length ? [{label: field.format(low), color: at(0)}, {label: field.format(high), color: at(1)}] : [], gradient: numbers.length > 0};
}

export function matches(t, filters) {
  const byField = new Map();
  filters.forEach(f => byField.set(f.field, [...(byField.get(f.field) || []), f.value]));
  return [...byField].every(([key, wanted]) => FIELD[key] && FIELD[key].values(t).some(v => wanted.includes(v)));
}
