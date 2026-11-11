import { mountMetrics, unmountMetrics } from "/metrics.js";
import {
  workerStatus as deriveWorkerStatus,
  coordinationSignature,
  queueTaskState,
} from "/status.js";
const $ = (s) => document.querySelector(s);
const routeParams = new URLSearchParams(location.search);
const globalScope = routeParams.get("scope") === "global";
const selectedProject = routeParams.get("project") || "default";
const apiBase = "/api/projects/" + encodeURIComponent(selectedProject);
const systemTheme = matchMedia("(prefers-color-scheme: dark)");
let theme =
  localStorage.getItem("relay-theme") ||
  (systemTheme.matches ? "dark" : "light");
document.documentElement.dataset.theme = theme;
function setTheme(value) {
  theme = value;
  document.documentElement.dataset.theme = value;
  localStorage.setItem("relay-theme", value);
}
systemTheme.addEventListener("change", (e) => {
  if (!localStorage.getItem("relay-theme")) {
    theme = e.matches ? "dark" : "light";
    document.documentElement.dataset.theme = theme;
    if (!editingSettings()) render();
  }
});

const esc = (v) =>
  String(v ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
const paths = {
  board: "M3 3h18v18H3z M9 3v18 M15 3v18",
  list: "M8 6h13 M8 12h13 M8 18h13 M3 6h.01 M3 12h.01 M3 18h.01",
  robot:
    "M6 8h12a2 2 0 012 2v9H4v-9a2 2 0 012-2z M12 8V4 M10 4h4 M8 12v2 M16 12v2 M9 17h6 M2 12v4 M22 12v4",
  layers: "M12 3l10 5-10 5L2 8z M2 12l10 5 10-5 M2 16l10 5 10-5",
  workflow: "M4 3h6v6H4z M14 15h6v6h-6z M7 9v8h7 M17 8v7 M10 6h7v2",
  activity: "M3 12h4l3-8 4 16 3-8h4",
  chart: "M4 20V10 M10 20V4 M16 20v-8 M22 20H2",
  settings:
    "M12 8a4 4 0 100 8 4 4 0 000-8 M9 3h6l1 3 3 1 2 5-2 5-3 1-1 3H9l-1-3-3-1-2-5 2-5 3-1z",
  plus: "M12 5v14 M5 12h14",
  close: "M6 6l12 12 M18 6L6 18",
  search: "M21 21l-5-5 M18 10a8 8 0 11-16 0 8 8 0 0116 0",
  arrow: "M4 12h16 M14 6l6 6-6 6",
  chevron: "M9 5l7 7-7 7",
  down: "M6 9l6 6 6-6",
  play: "M7 4l14 8-14 8z",
  pause: "M7 4v16 M17 4v16",
  check: "M5 12l4 4L20 5",
  clock: "M12 7v5l4 2 M22 12a10 10 0 11-20 0 10 10 0 0120 0",
  cycle: "M20 7a9 9 0 10.5 9 M20 3v5h-5",
  file: "M14 2H5v20h14V7z M14 2v6h5 M8 12h8 M8 16h6",
  code: "M8 6l-6 6 6 6 M16 6l6 6-6 6 M14 3l-4 18",
  flag: "M5 21V3 M5 3h14l-3 5 3 5H5",
  bolt: "M13 2L4 14h7l-1 8 10-13h-7z",
  lock: "M6 10h12v11H6z M8 10V6a4 4 0 018 0v4",
  download: "M12 3v12 M7 10l5 5 5-5 M4 16v5h16v-5",
  folder: "M2 6h8l2 3h10v11H2z",
  back: "M9 5l-7 7 7 7 M2 12h12a7 7 0 017 7",
  edit: "M15 4l5 5 M3 21l1-6L17 2l5 5L9 20z",
  terminal: "M4 6l6 6-6 6 M13 18h7",
  info: "M12 11v6 M12 7h.01 M22 12a10 10 0 11-20 0 10 10 0 0120 0",
  archive: "M3 3h18v5H3z M5 8v13h14V8 M9 12h6",
  diamond: "M12 2l10 10-10 10L2 12z",
};
const icon = (name) =>
  `<svg aria-hidden="true" viewBox="0 0 24 24"><path d="${paths[name] || paths.file}"/></svg>`;
const stageNames = {
  backlog: "Backlog",
  planning: "Planning",
  implementation: "In progress",
  review: "Review",
  done: "Done",
};
const stages = Object.keys(stageNames);
stageNames.verification = "Review (previous workflow)";
const stageShort = {
  planning: "Planner",
  implementation: "Builder",
  review: "Reviewer",
};
const typeNames = {
  feature: "Feature",
  bug: "Bug",
  research: "Research",
  refactor: "Refactor",
  documentation: "Documentation",
  operations: "Operations",
};
const typeIcons = {
  feature: "check",
  bug: "bolt",
  research: "search",
  refactor: "code",
  documentation: "file",
  operations: "settings",
};
const pageNames = {
  board: "Task board",
  epics: "Epics",
  agents: "Agents",
  workflow: "Workflow",
  activity: "Activity",
  insights: "Metrics",
  settings: "Workspace settings",
  archive: "Archive",
  overview: "All workspaces",
  "global-settings": "Global settings",
  queue: "Queue & blockers",
};
const localPages = ["board", "epics", "queue", "agents", "insights", "activity", "archive", "settings"];
const globalPages = ["overview", "global-settings"];
function validPage(value, global = globalScope) {
  if (value === "workflow") value = "settings";
  if (value === "metrics") value = "insights";
  return (global ? globalPages : localPages).includes(value) ? value : global ? "overview" : "board";
}
const rememberedPage = localStorage.getItem("relay-page:" + selectedProject) || localStorage.getItem("relay-page");
const requestedPage = routeParams.get("page") || (globalScope ? "overview" : rememberedPage);
const ui = {
  page: validPage(requestedPage),
  settingsSection: requestedPage === "workflow" ? "workflow" : ["general", "workflow", "data"].includes(routeParams.get("section")) ? routeParams.get("section") : "general",
  settingsDirty: false,
  loading: true,
  error: "",
  boardStage: "implementation",
  queueSearch: "",
  view: "board",
  search: "",
  type: "",
  epic: "",
  status: "",
  selected: null,
  tab: "overview",
  dialog: null,
  dialogData: null,
  connected: true,
};
let data = {
  tasks: [],
  agents: [],
  attempts: [],
  events: [],
  artifacts: [],
  settings: { paused: true, stage_agents: {} },
  queue_reasons: {},
};
let overviewData = null, workspaceMetrics = null, serverSettings = null;
let boot = {},
  refreshBusy = false,
  toastTimer,
  renderedRevision = -1,
  renderedCoordination = "";
const date = (value) =>
  value
    ? new Date(value).toLocaleString([], {
        month: "short",
        day: "numeric",
        hour: "numeric",
        minute: "2-digit",
      })
    : "—";
const duration = (ms) =>
  ms == null
    ? "—"
    : ms < 1000
      ? "<1s"
      : ms < 60000
        ? `${Math.round(ms / 1000)}s`
        : ms < 3600000
          ? `${Math.floor(ms / 60000)}m ${Math.floor((ms % 60000) / 1000)}s`
          : `${(ms / 3600000).toFixed(1)}h`;
const ago = (t) => {
  const m = Math.max(0, Math.floor((Date.now() - Date.parse(t)) / 60000));
  return m < 1
    ? "just now"
    : m < 60
      ? `${m}m ago`
      : m < 1440
        ? `${Math.floor(m / 60)}h ago`
        : `${Math.floor(m / 1440)}d ago`;
};
const initials = (name) =>
  String(name || "?")
    .replace("Demo · ", "")
    .split(" ")
    .map((s) => s[0])
    .slice(0, 2)
    .join("")
    .toUpperCase();
const num = (n) => (n == null ? "Unknown" : n.toLocaleString());
const tag = (name, cls = "") =>
  `<span class="tag ${esc(cls)}">${esc(name)}</span>`;
const btn = (label, action, ico = "", cls = "", attrs = "") =>
  `<button class="btn ${cls}" data-action="${action}" ${attrs}>${ico ? icon(ico) : ""}${label}</button>`;
const priority = (n) =>
  `<span class="priority p${n}" title="Priority ${n}"><i></i><i></i><i></i></span>`;
function toast(message) {
  $("#toast").textContent = message;
  $("#toast").classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => $("#toast").classList.remove("show"), 5000);
}
async function api(
  path,
  method = "GET",
  body,
  approvalCode = "",
  retried = false,
) {
  const response = await fetch(
    (["/projects", "/overview", "/global/settings"].includes(path) || (globalScope && path === "/bootstrap") ? "/api" : apiBase) + path,
    {
      method,
      headers: {
        "Content-Type": "application/json",
        "x-relay-token": boot.token || "",
        ...(approvalCode ? { "x-relay-approval": approvalCode } : {}),
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    },
  );
  const result = await response
    .json()
    .catch(() => ({ error: `Request failed (${response.status})` }));
  if (
    response.status === 403 &&
    !retried &&
    String(result.error).includes("Missing local session token")
  ) {
    boot = await api("/bootstrap");
    return api(path, method, body, approvalCode, true);
  }
  if (!response.ok) {
    const error = new Error(result.error || `Request failed (${response.status})`);
    error.status = response.status;
    throw error;
  }
  return result;
}
function editingSettings() {
  return ui.settingsDirty || !!document.activeElement?.closest("#settings-form,#workflow-form");
}
async function refresh(force = false) {
  if (refreshBusy) return;
  refreshBusy = true;
  try {
    if (globalScope) {
      let changed;
      if (ui.page === "global-settings") {
        const next = await api("/global/settings");
        changed = JSON.stringify(next) !== JSON.stringify(serverSettings);
        serverSettings = next;
        data.projects = await api("/projects");
      } else {
        const next = await api("/overview");
        changed = JSON.stringify(next.metrics) !== JSON.stringify(overviewData?.metrics) || JSON.stringify(next.workspaces) !== JSON.stringify(overviewData?.workspaces) || JSON.stringify(next.server) !== JSON.stringify(serverSettings);
        overviewData = next;
        serverSettings = next.server;
        data.projects = next.workspaces;
      }
      const firstLoad = ui.loading || !ui.connected;
      ui.connected = true;
      ui.loading = false;
      ui.error = "";
      if ((force || changed || firstLoad) && !ui.dialog && (force || !document.activeElement?.matches("input,textarea,select"))) render();
      return;
    }
    const next = await api("/state");
    if (boot.build_id && next.coordinator?.build_id && boot.build_id !== next.coordinator.build_id && !ui.dialog && !editingSettings() && !document.activeElement?.matches("input,textarea,select")) {
      location.reload();
      return;
    }
    const changed = next.revision !== renderedRevision || !ui.connected || coordinationSignature(next) !== renderedCoordination || JSON.stringify(next.projects) !== JSON.stringify(data.projects);
    data = next;
    if (ui.page === "insights") workspaceMetrics = await api("/metrics");
    ui.connected = true;
    ui.loading = false;
    ui.error = "";
    if (force || changed || ui.page === "insights") {
      if (!editingSettings() && (!document.activeElement?.matches("input,textarea,select") || force)) render();
      if (ui.selected && !ui.dialog && !document.activeElement?.matches("input,textarea")) renderDrawer();
    }
  } catch (e) {
    ui.connected = false;
    ui.loading = false;
    ui.error = e.status === 404 ? "This workspace is unavailable. Choose another workspace to continue." : e.message;
    if (!editingSettings() && !ui.dialog) render();
    else toast("Connection lost. Your unsaved form is preserved.");
  } finally {
    refreshBusy = false;
  }
}
function pageUrl(page, project = selectedProject, section = ui.settingsSection) {
  const global = globalPages.includes(page);
  const params = new URLSearchParams(global ? { scope: "global", page } : { project, page: validPage(page, false) });
  if (!global && page === "settings") params.set("section", section);
  return "/?" + params.toString();
}
function navigate(page, section) {
  if (globalPages.includes(page) !== globalScope) {
    location.assign(pageUrl(page));
    return;
  }
  if (page === "workflow") { page = "settings"; section = "workflow"; }
  ui.page = validPage(page);
  if (section) ui.settingsSection = section;
  if (!globalScope) localStorage.setItem("relay-page:" + selectedProject, ui.page);
  history.pushState({}, "", pageUrl(ui.page));
  ui.selected = null;
  ui.dialog = null;
  ui.settingsDirty = false;
  $("#overlay").innerHTML = "";
  render();
  refresh(true);
}
window.addEventListener("popstate", () => location.reload());
function filteredTasks() {
  return data.tasks.filter(
    (t) =>
      !t.archived &&
      t.kind !== "epic" &&
      (!ui.search ||
        `${t.key} ${t.title} ${t.tags.join(" ")}`
          .toLowerCase()
          .includes(ui.search.toLowerCase())) &&
      (!ui.type || t.task_type === ui.type) &&
      (!ui.status || t.status === ui.status) &&
      (!ui.epic ||
        t.parent_id === ui.epic ||
        data.tasks.find((p) => p.id === t.parent_id)?.parent_id === ui.epic),
  );
}
function taskAgent(t) {
  const active = data.attempts.find((a) => a.id === t.current_attempt);
  if (active) return { name: active.agent_name, id: active.agent_id };
  const assigned = data.agents.find(
    (a) => a.worker?.assignment?.task_id === t.id,
  );
  if (assigned) return assigned;
  const reported = data.agents.find(
    (a) => a.presence?.task_id === t.id && workerStatus(a) === "running",
  );
  if (reported) return reported;
  const id = t.assignments[t.stage] || data.settings.stage_agents[t.stage];
  return (
    data.agents.find((a) => a.id === id) ||
    data.agents.find(
      (a) =>
        a.enabled &&
        a.stages.includes(t.stage) &&
        (a.adapter !== "demo" || t.tags.includes("demo")),
    )
  );
}
function metrics() {
  const tasks = data.tasks.filter((t) => !t.archived && t.kind !== "epic");
  const done = tasks.filter((t) => t.status === "completed").length;
  const active = data.attempts.filter((a) => a.status === "running");
  return {
    tasks,
    done,
    active,
    sendbacks: tasks.reduce((n, t) => n + t.sendbacks, 0),
    totalTime: data.attempts.reduce((n, a) => n + (a.duration_ms || 0), 0),
  };
}
function workspaceName() { return data.settings.project_name || boot.project_name || "Workspace"; }
function navItem(page) {
  const ico = ({ overview: "layers", "global-settings": "settings", epics: "layers", agents: "robot", insights: "chart" })[page] || page;
  return `<button class="nav-item ${ui.page === page ? "active" : ""}" data-page="${page}" aria-label="${pageNames[page]}" title="${pageNames[page]}" ${ui.page === page ? 'aria-current="page"' : ""}>${icon(ico)}<span>${pageNames[page]}</span></button>`;
}
function shell(content) {
  const name = workspaceName();
  return `<div class="layout"><aside class="sidebar"><div class="brand"><span class="brand-mark">R</span><span>relay<span class="muted">.</span></span></div><p class="small-label">GLOBAL</p>${globalPages.map(navItem).join("")}<div class="nav-separator"></div><div class="workspace-switch"><span class="workspace-icon">${icon("layers")}</span><select id="project-switch" aria-label="Workspace">${projectOptions()}</select></div>${globalScope ? '<p class="scope-hint">Choose a workspace to manage its work and settings.</p>' : `<p class="small-label scope-name" title="${esc(name)}">${esc(name)}</p>${localPages.map(navItem).join("")}`}<div class="sidebar-foot"><small>Relay ${esc(boot.version)}</small></div></aside><main class="main"><header class="topbar"><select id="project-switch-mobile" class="mobile-project" aria-label="Workspace and global views">${projectOptions()}</select><span class="breadcrumb-scope">${globalScope ? "Global" : esc(name)}</span>${icon("chevron")}<strong>${pageNames[ui.page]}</strong><div class="topbar-end"><span class="connection"><span class="status-dot"></span>${ui.connected ? "Connected" : "Disconnected"}</span><button class="theme-toggle" data-action="toggle-theme" aria-label="${theme === "dark" ? "Switch to light mode" : "Switch to dark mode"}" title="${theme === "dark" ? "Light mode" : "Dark mode"}">${theme === "dark" ? "☀" : "☾"}</button></div></header><div class="content ${ui.page === "board" ? "board-page" : ""}">${content}</div></main></div>`;
}
function heading(title, sub, actions = "") {
  return `<div class="heading-row"><div><h1>${esc(title)}</h1>${sub ? `<p class="subtitle">${esc(sub)}</p>` : ""}</div><div class="actions">${actions}</div></div>`;
}
function stat(label, value, foot, ico, cls = "") {
  return `<div class="stat"><div class="stat-top">${label}${icon(ico)}</div><div class="stat-value">${value}</div><div class="stat-foot ${cls}">${foot}</div></div>`;
}
function projectOptions() {
  const projects = data.projects || boot.projects || [];
  const localPage = globalScope ? "board" : ui.page;
  return globalPages.map(p => `<option value="${esc(pageUrl(p))}" ${globalScope && ui.page === p ? "selected" : ""}>${pageNames[p]}</option>`).join("") +
    '<optgroup label="Workspaces">' + projects.map(p => `<option value="${esc(pageUrl(localPage, p.id))}" ${!globalScope && p.id === selectedProject ? "selected" : ""}>${esc(p.name)}</option>`).join("") + '</optgroup><option value="__add">New workspace…</option>';
}
function summaryStats(m) {
  if (!m) return '<p class="muted">Metrics are loading…</p>';
  return `<div class="stats scope-stats">${stat("Tasks", num(m.tasks), "Excludes epics and archived work", "board")}${stat("Completed", num(m.completed), "Of current tasks", "check")}${stat("Operational issues", num(m.attention), "Blocked, needs input or failed", "info")}${stat("Running attempts", num(m.running_attempts), "Across execution stages", "bolt")}</div>`;
}
function usagePanel(m) {
  if (!m) return "";
  const usage = (label, value, count, money = false) => `<div><small>${label}</small><strong>${value == null ? "Unavailable" : money ? "$" + value.toFixed(4) : num(value)}</strong><span class="metric-coverage">${count || 0} / ${m.attempts} attempts reporting${count > 0 && count < m.attempts ? " · Partial total" : ""}</span></div>`;
  return `<div class="panel"><h2>Execution & usage</h2><p class="muted">All recorded attempts, including archived work. Values are worker-reported; missing usage is not counted as zero.</p><div class="detail-grid usage-grid"><div><small>Attempts</small>${num(m.attempts)}</div><div><small>Failed attempts</small>${num(m.failed_attempts)}</div><div><small>Execution time</small>${duration(m.duration_ms)}</div><div><small>Sendbacks</small>${num(m.sendbacks)}</div>${usage("Input tokens", m.input_tokens, m.input_token_attempts)}${usage("Output tokens", m.output_tokens, m.output_token_attempts)}${usage("Reported cost", m.cost_usd, m.cost_attempts, true)}</div></div>`;
}
function overview() {
  const all = overviewData;
  if (!all) return heading("All workspaces", "Loading workspace summaries…");
  return heading("All workspaces", "Progress, attention and recorded usage across every workspace on this server.", btn("New workspace", "new-workspace", "plus", "primary")) +
    (all.server.draining ? '<div class="notice info">Global maintenance is enabled. New work is not being admitted in any workspace. <button class="text-link" data-page="global-settings">Manage maintenance</button></div>' : "") + summaryStats(all.metrics) +
    `<div class="panel"><h2>Workspaces <span class="muted">${all.workspaces.length}</span></h2><div class="table-wrap"><table class="workspace-table"><thead><tr><th>Workspace</th><th>Coordination</th><th>Tasks</th><th>Issues</th><th>Running</th><th>Launch capacity</th><th></th></tr></thead><tbody>${all.workspaces.map(p => `<tr><td><a href="${esc(pageUrl("board", p.id))}">${esc(p.name)}</a>${p.coordinator?.error ? `<small class="error-text">${esc(p.coordinator.error)}</small>` : ""}</td><td>${all.server.draining ? "Maintenance" : p.paused ? "Paused" : "Enabled"}</td><td>${num(p.metrics.tasks)}</td><td><a href="${esc(pageUrl("queue", p.id))}" aria-label="${esc(p.name)}: ${p.metrics.attention} tasks with operational issues">${num(p.metrics.attention)}</a></td><td>${num(p.metrics.running_attempts)}</td><td>${num(p.concurrency)}</td><td><a class="text-link" href="${esc(pageUrl("insights", p.id))}">Metrics</a> · <a class="text-link" href="${esc(pageUrl("settings", p.id, "general"))}">Settings</a></td></tr>`).join("") || '<tr><td colspan="7">No workspaces yet. Create one to begin.</td></tr>'}</tbody></table></div><p class="muted">Launch capacity is configured separately in each workspace. Workspace pause preserves running attempts.</p></div>` + usagePanel(all.metrics) + `<p class="muted">Updated ${date(all.generated_at)}</p>`;
}
function globalSettings() {
  const server = serverSettings;
  if (!server) return heading("Global settings", "Loading server settings…");
  return heading("Global settings", "Host controls apply to every workspace. Workflow policies remain in Workspace settings.") +
    `<div class="panel"><h2>Maintenance</h2><p>Stop new admissions across all workspaces while running attempts finish. Workspace pause preferences are preserved. Maintenance clears when the server restarts.</p><p><strong>${server.draining ? "Maintenance enabled" : "Accepting new work"}</strong></p>${btn(server.draining ? "Resume admissions in all workspaces" : "Enable maintenance for all workspaces", "toggle-maintenance", server.draining ? "play" : "pause")}</div>` +
    `<div class="panel"><h2>Appearance</h2><p>Theme is shared across workspaces in this browser only.</p><div class="actions"><button class="btn ${theme === "light" ? "primary" : ""}" data-action="set-theme" data-theme="light">Light</button><button class="btn ${theme === "dark" ? "primary" : ""}" data-action="set-theme" data-theme="dark">Dark</button></div></div>` +
    `<div class="panel"><h2>Server</h2><div class="detail-grid"><div><small>Version</small>${esc(server.version)}</div><div><small>Build</small>${esc(server.build_id)}</div><div><small>Host data directory</small><code class="path-value">${esc(server.data_dir)}</code></div></div><p class="muted">Backups and exports are per workspace under Workspace settings → Data. Preserve the complete host data directory to restore all workspaces.</p></div>`;
}
function workerStatus(agent, state = data) {
  return deriveWorkerStatus(agent, state);
}
function stats() {
  return '<div id="metrics-root"></div>';
}
function currentMetrics() {
  const m = metrics(),
    agents = data.agents.filter((a) => a.adapter !== "demo");
  return {
    tasks: m.tasks.length,
    attention: m.tasks.filter((t) =>
      ["blocked", "needs_input", "failed"].includes(t.status),
    ).length,
    active: agents.filter((a) => workerStatus(a) === "running").length,
    unknown: agents.filter((a) => workerStatus(a) === "unknown").length,
    connected: agents.length,
    managed: m.active.filter((a) => !a.external).length,
    capacity: data.settings.concurrency,
    done: m.done,
    sendbacks: m.sendbacks,
    attempts: data.attempts.length,
  };
}
function coordinationStatus() {
  const waiting = data.tasks.filter(
    (t) =>
      !t.archived &&
      t.kind !== "epic" &&
      t.stage !== "done" &&
      (t.blocked_reason || data.queue_reasons[t.id]),
  ).length;
  return (
    '<div class="coordination-status"><span>' +
    esc(data.coordinator?.error || dispatchLabel()) +
    '</span><button class="text-link" data-page="queue">' +
    waiting +
    " waiting · View queue</button></div>"
  );
}
function dispatchLabel() {
  if (data.settings.paused) return "Automatic dispatch paused";
  return (
    {
      running: "Tasks running",
      dispatching: "Dispatching eligible tasks",
      assigned: "Assignments waiting for pickup",
      waiting_for_workers: "Waiting for a worker to claim work",
      blocked: "Waiting on task blockers",
      idle: "No work queued",
    }[data.dispatch?.mode] || "Automatic dispatch enabled"
  );
}
function queue() {
  const tasks = data.tasks.filter(
    (t) =>
      !t.archived &&
      t.kind !== "epic" &&
      t.stage !== "done" &&
      (!ui.queueSearch ||
        (
          t.key +
          " " +
          t.title +
          " " +
          t.blocked_reason +
          " " +
          (data.queue_reasons[t.id] || "")
        )
          .toLowerCase()
          .includes(ui.queueSearch.toLowerCase())),
  );
  return (
    heading("Queue & blockers", "") +
    '<div class="coordination-status"><span>' +
    esc(dispatchLabel()) +
    "</span><span>" +
    (data.dispatch?.active_attempts ?? 0) +
    " active attempts · " +
    (data.dispatch?.eligible ?? 0) +
    " eligible tasks · " +
    (data.dispatch?.automatic_workers ?? 0) +
    " automatic workers</span></div>" +
    '<div class="toolbar"><label class="search">' +
    icon("search") +
    '<input id="queue-search" aria-label="Search queue" placeholder="Search queue…" value="' +
    esc(ui.queueSearch) +
    '"></label><span>' +
    tasks.length +
    ' tasks</span></div><div id="queue-table">' +
    queueTable(tasks) +
    "</div>"
  );
}
function queueTable(tasks) {
  return (
    '<div class="table-wrap"><table><thead><tr><th>Task</th><th>Stage</th><th>Status</th><th>Reason</th><th>Next action</th></tr></thead><tbody>' +
    tasks
      .map((t) => {
        const { status, reason, next, target } = queueTaskState(t, data);
        return (
          '<tr><td><button class="queue-task text-link" data-task="' +
          t.id +
          '"><small>' +
          esc(t.key) +
          '<span class="queue-mobile-status"> · ' +
          esc(stageNames[t.stage] + " · " + status) +
          "</span>" +
          "</small>" +
          esc(t.title) +
          "</button></td><td>" +
          stageNames[t.stage] +
          "</td><td>" +
          esc(status) +
          "</td><td>" +
          esc(reason) +
          "</td><td>" +
          btn(next, "queue-open", "", "small", 'data-id="' + (target || t.id) + '"') +
          "</td></tr>"
        );
      })
      .join("") +
    "</tbody></table></div>"
  );
}
function board() {
  const tasks = filteredTasks();
  return (
    heading(
      "Task board",
      "",
      btn(
        data.settings.paused ? "Start coordination" : "Pause coordination",
        "toggle-scheduler",
        data.settings.paused ? "play" : "pause",
      ) + btn("New task", "new-task", "plus", "primary"),
    ) +
    stats() +
    coordinationStatus() +
    `<div class="toolbar"><label class="search">${icon("search")}<input id="task-search" placeholder="Search tasks…" aria-label="Search tasks" value="${esc(ui.search)}"><span class="keycap">/</span></label><select class="filter" id="type-filter" aria-label="Filter by task type"><option value="">All types</option>${Object.entries(
      typeNames,
    )
      .map(
        ([v, l]) =>
          `<option value="${v}" ${ui.type === v ? "selected" : ""}>${l}</option>`,
      )
      .join(
        "",
      )}</select><select class="filter" id="epic-filter" aria-label="Filter by epic"><option value="">All epics</option>${data.tasks
      .filter((t) => t.kind === "epic" && !t.archived)
      .map(
        (t) =>
          `<option value="${t.id}" ${ui.epic === t.id ? "selected" : ""}>${esc(t.title)}</option>`,
      )
      .join(
        "",
      )}</select><select class="filter" id="status-filter" aria-label="Filter by status"><option value="">All statuses</option>${["ready", "running", "paused", "blocked", "needs_input", "failed", "completed", "cancelled"].map((v) => `<option value="${v}" ${ui.status === v ? "selected" : ""}>${v.replace("_", " ")}</option>`).join("")}</select><div class="toolbar-right"><span>${tasks.length} tasks</span><div class="view-switch"><button data-view="board" class="${ui.view === "board" ? "active" : ""}" aria-label="Board view">${icon("board")}</button><button data-view="list" class="${ui.view === "list" ? "active" : ""}" aria-label="List view">${icon("list")}</button></div></div></div><div id="task-view">${ui.view === "board" ? kanban(tasks) : taskTable(tasks)}</div>`
  );
}
function kanban(tasks) {
  return `<nav class="stage-tabs" aria-label="Board stage">${stages.map((stage) => `<button data-action="board-stage" data-stage="${stage}" class="${ui.boardStage === stage ? "active" : ""}">${stageNames[stage]} <span>${tasks.filter((t) => t.stage === stage).length}</span></button>`).join("")}</nav><div class="board">${stages
    .map(
      (stage) =>
        `<section class="column stage-${stage}" data-drop-stage="${stage}" data-active="${ui.boardStage === stage}" aria-label="${stageNames[stage]}"><div class="column-head"><span class="dot"></span>${stageNames[stage]}<span class="column-count">${tasks.filter((t) => t.stage === stage).length}</span><button data-action="new-task" aria-label="Create task">${icon("plus")}</button></div>${tasks
          .filter((t) => t.stage === stage)
          .sort((a, b) => a.priority - b.priority)
          .map(taskCard)
          .join(
            "",
          )}<button class="column-add" data-action="new-task">${icon("plus")} Add task</button></section>`,
    )
    .join("")}</div>`;
}
function taskCard(t) {
  const agent = taskAgent(t);
  const attempts = data.attempts.filter((a) => a.task_id === t.id).length;
  return `<article class="task-card" draggable="true" data-task="${t.id}" tabindex="0" role="button" aria-label="${esc(t.key + " " + t.title)}"><div class="card-top">${icon(typeIcons[t.task_type])}<span>${t.key}</span>${t.kind === "subtask" ? icon("layers") : ""}<span class="card-more">···</span></div><h3>${esc(t.title)}</h3>${t.status !== "ready" && t.status !== "completed" ? `<div class="card-status ${t.status}">${icon(t.status === "running" ? "bolt" : "info")}${esc(t.status.replace("_", " "))}</div>` : ""}<div class="card-tags">${t.recovery_of ? tag("Coordinator recovery", "demo") : ""}${tag(typeNames[t.task_type], t.task_type)}${t.tags
    .slice(0, 2)
    .map((v) => tag(v, v === "demo" ? "demo" : ""))
    .join(
      "",
    )}</div><div class="card-footer">${priority(t.priority)}<span class="points" title="Complexity points">${t.complexity}</span>${attempts ? `<span title="Attempts">${attempts} run${attempts === 1 ? "" : "s"}</span>` : ""}${t.sendbacks ? `<span title="Sendbacks">↩ ${t.sendbacks}</span>` : ""}<span class="agent-chip"><span>${agent ? esc(agent.name.replace("Demo · ", "")) : t.stage === "done" ? "Complete" : "Unassigned"}</span><i class="agent-dot">${agent ? esc(initials(agent.name)) : t.stage === "done" ? "✓" : "—"}</i></span></div></article>`;
}
function taskTable(tasks) {
  return `<div class="table-wrap"><table><thead><tr><th>Task</th><th>Stage</th><th>Status</th><th>Type</th><th>Complexity</th><th>Agent</th></tr></thead><tbody>${tasks.map((t) => `<tr data-task="${t.id}"><td class="task-title-cell"><small>${t.key}</small>${esc(t.title)}</td><td>${stageNames[t.stage]}</td><td>${esc(t.status.replace("_", " "))}</td><td>${tag(typeNames[t.task_type], t.task_type)}</td><td>${priority(t.priority)} &nbsp; ${t.complexity}</td><td>${esc(taskAgent(t)?.name || "Unassigned")}</td></tr>`).join("") || '<tr><td colspan="6">No tasks match your filters.</td></tr>'}</tbody></table></div>`;
}
function epics() {
  const epics = data.tasks.filter((t) => t.kind === "epic" && !t.archived);
  return (
    heading("Epics", "", btn("New epic", "new-epic", "plus", "primary")) +
    (epics.length
      ? epics
          .map((e) => {
            const children = data.tasks.filter(
              (t) => t.parent_id === e.id && !t.archived,
            );
            const done = children.filter(
              (t) => t.status === "completed",
            ).length;
            return `<article class="epic-card" data-task="${e.id}"><div class="epic-top"><div><small>${e.key} · EPIC</small><h3>${esc(e.title)}</h3></div>${tag(`${done} / ${children.length} complete`)}</div><p>${esc(e.description)}</p><progress max="${children.length || 1}" value="${done}" aria-label="Epic completion"></progress><div class="epic-tasks">${children.map((t) => `<button class="epic-task" data-task="${t.id}">${t.key} · ${esc(t.title)}</button>`).join("")}</div></article>`;
          })
          .join("")
      : empty(
          "layers",
          "No epics",
          "",
          btn("Create an epic", "new-epic", "plus", "primary"),
        ))
  );
}
function empty(ico, title, description, action = "") {
  return `<div class="empty"><div class="empty-icon">${icon(ico)}</div><h2>${title}</h2><p>${description}</p>${action}</div>`;
}
function agents() {
  const team = data.agents.filter((a) => a.adapter !== "demo");
  return (
    heading(
      "Agents",
      workspaceName() + " · Profiles and worker registrations for this workspace.",
      btn("Add command agent", "new-agent", "plus", "primary"),
    ) +
    (team.length
      ? '<div class="agent-grid">' +
        team
          .map((a) => {
            const status = workerStatus(a),
              runs = data.attempts.filter((r) => r.agent_id === a.id);
            const task = data.tasks.find(
              (t) =>
                t.id === (a.worker?.assignment?.task_id || a.presence?.task_id),
            );
            return (
              '<article class="agent-card"><div class="agent-card-head"><span class="agent-icon">' +
              icon("robot") +
              "</span><div><h3>" +
              esc(a.name) +
              '</h3><span class="muted">' +
              esc(a.model || "Model unavailable") +
              "</span></div></div><p>" +
              esc(
                a.worker
                  ? "Automatic worker"
                  : a.adapter === "external"
                    ? "External activity"
                    : "Automatic command agent",
              ) +
              "</p>" +
              (task
                ? '<button class="text-link" data-task="' +
                  task.id +
                  '">' +
                  esc(task.key + " · " + task.title) +
                  "</button>"
                : "") +
              '<div class="agent-meta">' +
              a.stages.map((s) => tag(stageNames[s])).join("") +
              '</div><div class="agent-bottom"><span class="pill ' +
              (status === "running" ? "running" : "") +
              '">' +
              esc(status) +
              '</span><span class="muted">' +
              runs.length +
              " attempts</span>" +
              (a.adapter === "external"
                ? ""
                : btn(
                    "Configure",
                    "edit-agent",
                    "",
                    "small",
                    'data-id="' + esc(a.id) + '"',
                  )) +
              "</div>" +
              (a.presence
                ? "<small>Last report " + ago(a.presence.seen_at) + "</small>"
                : "") +
              "</article>"
            );
          })
          .join("") +
        "</div>"
      : empty(
          "robot",
          "No connected agents",
          "",
          btn("Add command agent", "new-agent", "plus", "primary"),
        ))
  );
}
function routingSelect(stage, selected = "") {
  return `<select name="route_${stage}" aria-label="${stageNames[stage]} agent"><option value="">Any eligible agent</option>${data.agents
    .filter((a) => a.stages.includes(stage))
    .map(
      (a) =>
        `<option value="${a.id}" ${selected === a.id ? "selected" : ""}>${esc(a.name)}${a.enabled ? "" : " (disabled)"}</option>`,
    )
    .join("")}</select>`;
}
function workflow() {
  return (
    '<div class="panel"><h2>Workflow</h2><p class="muted">Routing and execution policy for this workspace. Concurrent launches limits local command launches; agent capacity remains agent-owned.</p><form id="workflow-form" data-revision="' + data.revision + '"><div class="form-grid">' +
    stages
      .slice(1, -1)
      .map(
        (stage) =>
          '<div class="field"><label>' +
          stageNames[stage] +
          " agent</label>" +
          routingSelect(stage, data.settings.stage_agents[stage]) +
          "</div>",
      )
      .join("") +
    field(
      "Concurrent launches",
      "concurrency",
      data.settings.concurrency,
      "number",
      'min="1" required',
    ) +
    field(
      "Retry delay (seconds)",
      "retry_delay_seconds",
      data.settings.retry_delay_seconds ?? 5,
      "number",
      'min="0" required',
    ) +
    '<label class="check-row"><input type="checkbox" name="independent_review" ' +
    (data.settings.independent_review ? "checked" : "") +
    '> Independent review</label><label class="check-row"><input type="checkbox" name="auto_advance" ' +
    (data.settings.auto_advance !== false ? "checked" : "") +
    '> Start ready backlog tasks and retry failures automatically</label></div><div class="form-error"></div><div class="form-actions"><button class="btn" type="button" data-action="reload-settings">Reload saved settings</button><button class="btn primary" type="submit">Save workflow</button></div></form></div>'
  );
}
function eventList(events) {
  return events
    .map(
      (e) =>
        `<div class="timeline-item"><span class="event-icon">${icon({ created: "plus", sendback: "back", attempt_started: "play", attempt_finished: "check", attempt_failed: "info", transition: "arrow", comment: "file" }[e.kind] || "activity")}</span><div><p>${esc(e.message)}</p><small>${esc(e.actor)}${e.task_id ? ` · <button class="text-link" data-task="${e.task_id}">${esc(data.tasks.find((t) => t.id === e.task_id)?.key || "task")}</button>` : ""}</small></div><span class="timeline-time" title="${date(e.at)}">${ago(e.at)}</span></div>`,
    )
    .join("");
}
function activity() {
  return (
    heading(
      "Activity",
      "Every decision and handoff, in one durable timeline.",
    ) +
    `<div class="panel">${eventList([...data.events].reverse().slice(0, 150)) || '<p class="muted">Your workspace history will appear here.</p>'}</div>`
  );
}
function insights() {
  const runs = data.attempts;
  return heading("Workspace metrics", workspaceName() + " · Progress and recorded usage for this workspace only.") + summaryStats(workspaceMetrics) + usagePanel(workspaceMetrics) +
    `<div class="panel"><h2>Attempts by stage</h2>${stages.slice(1,-1).map(s => { const count = runs.filter(r => r.stage === s).length; return `<div class="chart-row"><span class="chart-label">${stageNames[s]}</span><meter max="${Math.max(1,runs.length)}" value="${count}" aria-label="${stageNames[s]} attempts"></meter><span class="chart-value">${count}</span></div>`; }).join("")}</div><div class="panel"><h2>Agent results</h2><p class="muted">Agent profiles and worker registrations belong to this workspace.</p><div class="table-wrap"><table><thead><tr><th>Agent</th><th>Attempts</th><th>Completed</th><th>Sendbacks</th><th>Failed</th><th>Execution time</th></tr></thead><tbody>${data.agents.map(a => { const r = runs.filter(r => r.agent_id === a.id); return `<tr><td>${esc(a.name)}</td><td>${r.length}</td><td>${r.filter(r => r.status === "completed").length}</td><td>${r.filter(r => r.status === "send_back").length}</td><td>${r.filter(r => r.status === "failed").length}</td><td>${duration(r.reduce((n,a) => n + (a.duration_ms || 0),0))}</td></tr>`; }).join("") || '<tr><td colspan="6">No agents yet.</td></tr>'}</tbody></table></div></div>`;
}
function settings() {
  const tabs = `<nav class="settings-tabs" aria-label="Workspace settings sections">${["general","workflow","data"].map(s => `<button data-action="settings-section" data-section="${s}" class="${ui.settingsSection === s ? "active" : ""}" ${ui.settingsSection === s ? 'aria-current="page"' : ""}>${s[0].toUpperCase()+s.slice(1)}</button>`).join("")}</nav>`;
  let content;
  if (ui.settingsSection === "workflow") content = workflow();
  else if (ui.settingsSection === "data") content = `<div class="panel"><h2>Backups & export</h2><p>Includes only ${esc(workspaceName())}: tasks, agents, settings, artifacts and history. Execution files are stored separately.</p><div class="actions">${btn("Create workspace backup", "backup", "download")}<a class="btn" href="${apiBase}/export" download>Export workspace JSON</a></div></div>`;
  else content = `<div class="panel"><form id="settings-form" data-revision="${data.revision}"><h2>General</h2><div class="form-grid"><div class="field"><label for="workspace-name">Workspace name</label><input id="workspace-name" name="project_name" value="${esc(data.settings.project_name)}" required></div><div class="field"><label for="workspace-data-dir">Workspace data directory</label><input id="workspace-data-dir" value="${esc(boot.data_dir)}" readonly></div></div><div class="form-error" role="alert"></div><div class="form-actions"><button class="btn" type="button" data-action="reload-settings">Reload saved settings</button><button class="btn primary" type="submit">Save workspace settings</button></div></form></div><div class="panel"><h2>Coordination</h2><p>Pause new automatic dispatch in ${esc(workspaceName())}. Running attempts continue.</p>${btn(data.settings.paused ? "Resume workspace coordination" : "Pause workspace coordination", "toggle-scheduler", data.settings.paused ? "play" : "pause")}</div>`;
  return heading("Workspace settings", workspaceName() + " · Changes affect this workspace only.") + tabs + content;
}
function archive() { return heading("Archive", workspaceName() + " · Restore archived work with its history intact.") + archivePanel(); }
function archivePanel() {
  const tasks = data.tasks.filter(
    (t) => t.archived && !t.tags.includes("demo"),
  );
  return `<div class="panel"><h2>Archived tasks</h2>${tasks.length ? `<div class="table-wrap"><table><thead><tr><th>Task</th><th>Last stage</th><th></th></tr></thead><tbody>${tasks.map((t) => `<tr><td>${esc(t.key)} · ${esc(t.title)}</td><td>${stageNames[t.stage]}</td><td>${btn("Restore", "restore-task", "cycle", "small", `data-id="${t.id}"`)}</td></tr>`).join("")}</tbody></table></div>` : '<p class="muted">Nothing archived. Archived work can be restored here.</p>'}</div>`;
}
function render() {
  renderedRevision = data.revision;
  renderedCoordination = coordinationSignature(data);
  const pages = {
    queue,
    board,
    epics,
    agents,
    activity,
    archive,
    overview,
    "global-settings": globalSettings,
    insights,
    settings,
  };
  if (!pages[ui.page]) ui.page = "board";
  unmountMetrics();
  $("#app").innerHTML = shell(
    ui.loading ? heading("Loading Relay", "Connecting to the selected scope…") : ui.error ? `<div class="panel"><h1>${globalScope ? "Global view unavailable" : "Workspace unavailable"}</h1><p>${esc(ui.error)}</p><a class="btn" href="/?scope=global&page=overview">All workspaces</a> ${btn("Retry", "retry-load", "cycle")}</div>` : pages[ui.page](),
  );
  mountMetrics($("#metrics-root"), currentMetrics());
}
function openTask(id) {
  ui.selected = id;
  ui.tab = "overview";
  ui.dialog = null;
  renderDrawer();
}
function recoveryNotice(task) {
  if (task.recovery_of) {
    const original = data.tasks.find(t => t.id === task.recovery_of.task_id);
    return `<div class="notice info"><strong>Coordinator-owned recovery</strong><p>Agents diagnose, repair and independently review this execution blocker for <button class="text-link" data-task="${esc(task.recovery_of.task_id)}">${esc(original?.key || "original task")}</button>. ${task.recovery_of.resumed ? "The original task has resumed." : "Recovery continues until the original task can resume."}</p></div>`;
  }
  const repair = data.tasks.find(t => t.recovery_of?.task_id === task.id && !t.recovery_of.resumed && !t.archived);
  return repair ? `<div class="notice info"><strong>Coordinator-owned recovery</strong><p>Agents are resolving the execution blocker in <button class="text-link" data-task="${esc(repair.id)}">${esc(repair.key)} · ${esc(repair.title)}</button>.</p></div>` : "";
}
function renderDrawer() {
  const t = data.tasks.find((t) => t.id === ui.selected);
  if (!t) {
    $("#overlay").innerHTML = "";
    return;
  }
  const attempts = data.attempts.filter((a) => a.task_id === t.id);
  const artifacts = data.artifacts.filter((a) => a.task_id === t.id);
  const events = data.events.filter((e) => e.task_id === t.id);
  const agent = taskAgent(t);
  const reason = t.blocked_reason || data.queue_reasons[t.id];
  let body = "";
  if (ui.tab === "overview")
    body = `${recoveryNotice(t)}${t.tags.includes("demo") ? '<div class="notice info">Demo task. Its agents simulate outcomes and do not perform real work.</div>' : ""}${reason ? `<div class="notice">${esc(reason)}</div>` : ""}<div class="detail-section"><h3>Description</h3><div class="prose">${esc(t.description || "No description yet.")}</div></div><div class="detail-section"><h3>Acceptance criteria</h3><div class="prose">${esc(t.acceptance || "Add acceptance criteria before starting.")}</div></div>${t.constraints ? `<div class="detail-section"><h3>Constraints</h3><div class="prose">${esc(t.constraints)}</div></div>` : ""}<div class="detail-section"><h3>Execution details</h3><div class="detail-grid"><div><small>Assigned agent</small>${esc(agent?.name || "Any eligible agent")}</div><div><small>Complexity</small>${t.complexity} points · ${esc(t.estimates.at(-1)?.confidence || "medium")} confidence</div><div><small>Attempts this visit</small>${attempts.filter((a) => a.visit === t.visit).length} / ${t.max_attempts || "Unlimited"}</div><div><small>Sendbacks</small>${t.sendbacks} / ${t.max_sendbacks || "Unlimited"}</div><div><small>Execution timeout</small>${t.timeout_seconds ? `${t.timeout_seconds} seconds` : "Unlimited"}</div><div><small>Reported token budget</small>${t.token_budget ? num(t.token_budget) : "No limit"}</div><div><small>Execution directory</small>${esc(t.workspace || "Isolated run directory")}</div><div><small>Created</small>${date(t.created_at)}</div></div></div><div class="detail-section"><h3>Relationships</h3>${t.parent_id ? `<p>Parent: <button class="text-link" data-task="${t.parent_id}">${esc(data.tasks.find((p) => p.id === t.parent_id)?.title)}</button></p>` : ""}${t.dependencies.length ? `<p>Depends on: ${t.dependencies.map((d) => `<button class="text-link" data-task="${d}">${esc(data.tasks.find((t) => t.id === d)?.key)}</button>`).join(", ")}</p>` : '<p class="muted">No task dependencies.</p>'}${data.tasks
      .filter((c) => c.parent_id === t.id && !c.archived)
      .map(
        (c) =>
          `<p><button class="text-link" data-task="${c.id}">${c.key} · ${esc(c.title)}</button> <small class="muted">${stageNames[c.stage]}</small></p>`,
      )
      .join(
        "",
      )}${t.kind !== "subtask" ? btn(t.kind === "epic" ? "Add task" : "Add subtask", "new-child", "plus", "small", `data-parent="${t.id}"`) : ""}</div><details><summary>Estimate history (${t.estimates.length})</summary>${t.estimates.map((e) => `<p class="muted">${e.points} points · ${esc(e.confidence)} · ${esc(e.reason)}<br><small>${date(e.at)}</small></p>`).join("")}</details>`;
  if (ui.tab === "artifacts")
    body = artifacts.length
      ? [...artifacts]
          .reverse()
          .map(
            (a) =>
              `<div class="artifact"><div class="artifact-head">${icon("file")}${esc(a.name)}${a.stale ? tag("Superseded", "demo") : ""}<small>${stageNames[a.stage]} · visit ${a.visit}</small><button class="icon-btn" data-action="download-artifact" data-id="${a.id}" aria-label="Download artifact">${icon("download")}</button></div><div class="artifact-content">${esc(a.content)}</div></div>`,
          )
          .join("")
      : empty(
          "file",
          "The work will live here",
          "Plans, deliverables, and validation evidence are kept with the task.",
        );
  if (ui.tab === "attempts")
    body = attempts.length
      ? [...attempts]
          .reverse()
          .map(
            (a) =>
              `<div class="attempt"><div class="attempt-head"><h3>${esc(a.agent_name)} · ${stageNames[a.stage]}</h3><span class="pill ${a.status === "running" ? "running" : a.status === "completed" ? "" : "off"}">${esc(a.status)}</span></div><p>Visit ${a.visit} · attempt ${a.number} · ${esc(a.model || "Model not reported")}<br>${date(a.started_at)}</p><div class="attempt-metrics"><span>${duration(a.duration_ms ?? (a.status === "running" ? Date.now() - Date.parse(a.started_at) : null))}</span><span>In: ${num(a.usage.input_tokens)}</span><span>Out: ${num(a.usage.output_tokens)}</span><span>${esc(a.usage.source)}</span></div>${a.summary ? `<div class="prose">${esc(a.summary)}</div>` : ""}${a.error ? `<div class="error-text">${esc(a.error)}</div>` : ""}<details><summary>Input snapshot</summary><pre>${esc(JSON.stringify(a.input, null, 2))}</pre></details>${a.output ? `<details><summary>Structured result</summary><pre>${esc(JSON.stringify(a.output, null, 2))}</pre></details>` : ""}${a.log ? `<details><summary>Execution log</summary><pre>${esc(a.log)}</pre></details>` : ""}</div>`,
          )
          .join("")
      : empty(
          "play",
          "No attempts yet",
          "Start the task, then run a stage manually or start the scheduler.",
        );
  if (ui.tab === "history")
    body = `${eventList([...events].reverse())}<form id="comment-form" class="detail-section"><div class="field"><label>Add context or answer a question</label><textarea name="reason" placeholder="What should the next agent know?" required></textarea></div><div class="form-error"></div><div class="form-actions"><button class="btn primary">Add comment</button></div></form>`;
  let controls = "";
  if (t.human_approval)
    controls += t.approval
      ? btn("Reopen approval", "reopen-approval", "back")
      : btn("Record approval", "approve-gate", "check", "primary");
  else if (t.kind === "epic" && t.stage !== "done")
    controls += btn("Complete epic", "complete-stage", "check", "primary");
  else if (t.stage === "backlog" && !t.archived)
    controls += btn("Start task", "start-task", "play", "primary");
  else if (t.status === "ready" && t.stage !== "done")
    controls += btn("Run this stage", "run-task", "play", "primary");
  if (t.status === "running")
    controls += btn("Pause task", "pause-task", "pause");
  else if (
    ["paused", "failed", "blocked", "needs_input", "cancelled"].includes(
      t.status,
    ) &&
    t.stage !== "done"
  )
    controls += btn(
      t.status === "failed" ? "Retry stage" : "Resume task",
      "resume-task",
      "cycle",
      "primary",
    );
  if (!t.human_approval && stages.indexOf(t.stage) > 1)
    controls += btn("Send back", "send-back", "back");
  if (
    t.kind !== "epic" &&
    ["planning", "implementation", "review"].includes(t.stage) &&
    t.status !== "running" &&
    t.status !== "cancelled"
  )
    controls += btn("Complete manually", "complete-stage", "check");
  if (t.status !== "running" && t.stage !== "done")
    controls += btn("Edit", "edit-task", "edit");
  if (!t.archived)
    controls += btn("Archive", "archive-task", "archive", "quiet");
  if (t.archived)
    controls = btn(
      "Restore task",
      "restore-task",
      "cycle",
      "primary",
      `data-id="${t.id}"`,
    );
  $("#overlay").innerHTML =
    `<div class="backdrop drawer-backdrop" data-backdrop="true"><section class="drawer" role="dialog" aria-modal="true" aria-label="${esc(t.title)}"><div class="drawer-head"><div class="drawer-top"><span>${t.key} / ${esc(t.kind)} / revision ${t.revision}</span><button class="icon-btn" data-action="close-overlay" aria-label="Close task">${icon("close")}</button></div><h2>${esc(t.title)}</h2><div class="drawer-meta">${tag(stageNames[t.stage])}${tag(t.status.replace("_", " "), "demo")}${tag(typeNames[t.task_type], t.task_type)}${priority(t.priority)}<span class="muted">${t.complexity} points</span></div></div><nav class="drawer-tabs">${[
      ["overview", "Overview"],
      ["artifacts", `Artifacts ${artifacts.length}`],
      ["attempts", `Attempts ${attempts.length}`],
      ["history", "History"],
    ]
      .map(
        ([tab, label]) =>
          `<button data-tab="${tab}" class="${ui.tab === tab ? "active" : ""}">${label}</button>`,
      )
      .join(
        "",
      )}</nav><div class="drawer-body">${body}</div><footer class="detail-actions">${controls}</footer></section></div>`;
}
function modal(title, body, formId, submit = "Save") {
  return `<div class="backdrop" data-backdrop="true"><section class="modal" role="dialog" aria-modal="true" aria-label="${esc(title)}"><div class="modal-head"><h2>${title}</h2><button class="icon-btn" data-action="close-modal" aria-label="Close dialog">${icon("close")}</button></div><form id="${formId}">${body}<div class="form-error" role="alert"></div><div class="form-actions">${btn("Cancel", "close-modal", "", "", 'type="button"')}<button class="btn primary" type="submit">${submit}</button></div></form></section></div>`;
}
function field(label, name, value = "", type = "text", extra = "") {
  return `<div class="field"><label for="f-${name}">${label}</label><input id="f-${name}" name="${name}" type="${type}" value="${esc(value)}" ${extra}></div>`;
}
function textarea(label, name, value = "", help = "") {
  return `<div class="field full"><label for="f-${name}">${label}</label><textarea id="f-${name}" name="${name}">${esc(value)}</textarea>${help ? `<small>${help}</small>` : ""}</div>`;
}
function selectField(label, name, options, value, extra = "") {
  return `<div class="field"><label for="f-${name}">${label}</label><select id="f-${name}" name="${name}" ${extra}>${options.map(([v, l]) => `<option value="${esc(v)}" ${String(value) === String(v) ? "selected" : ""}>${esc(l)}</option>`).join("")}</select></div>`;
}
function openTaskForm(existing = null, kind = "task", parent = "") {
  ui.dialog = "task";
  ui.dialogData = existing;
  const t = existing || {
    title: "",
    description: "",
    acceptance: "",
    constraints: "",
    kind,
    task_type: "feature",
    priority: 2,
    complexity: 3,
    parent_id: parent,
    dependencies: [],
    tags: [],
    assignments: {},
    workspace: "",
    max_attempts: 0,
    max_sendbacks: 0,
    timeout_seconds: 0,
    token_budget: null,
  };
  const parents = data.tasks.filter(
    (p) =>
      !p.archived &&
      p.id !== t.id &&
      p.stage !== "done" &&
      ["epic", "task"].includes(p.kind),
  );
  const body = `<div class="form-grid"><div class="field full"><label for="task-title">Title</label><input id="task-title" name="title" placeholder="What needs to get done?" value="${esc(t.title)}" required autofocus></div>${existing ? "" : '<div class="field"><label class="check-row"><input type="checkbox" name="human_approval"> Require explicit human approval</label></div>'}${selectField(
    "Hierarchy",
    "kind",
    [
      ["epic", "Epic"],
      ["task", "Task"],
      ["subtask", "Subtask"],
    ],
    t.kind,
    existing ? "disabled" : "",
  )}${selectField("Type", "task_type", Object.entries(typeNames), t.task_type)}${selectField("Parent", "parent_id", [["", "No parent"], ...parents.map((p) => [p.id, `${p.key} · ${p.kind} · ${p.title}`])], t.parent_id || "")}${selectField(
    "Priority",
    "priority",
    [
      [1, "High"],
      [2, "Normal"],
      [3, "Low"],
      [4, "Someday"],
    ],
    t.priority,
  )}${textarea("Description", "description", t.description)}${textarea("Acceptance criteria", "acceptance", t.acceptance, "Describe what must be true before this task is complete.")}${selectField(
    "Complexity",
    "complexity",
    [1, 2, 3, 5, 8, 13].map((n) => [
      n,
      `${n} points${n === 13 ? " · consider splitting" : ""}`,
    ]),
    t.complexity,
  )}${selectField(
    "Estimate confidence",
    "estimate_confidence",
    [
      ["low", "Low"],
      ["medium", "Medium"],
      ["high", "High"],
    ],
    t.estimates?.at(-1)?.confidence || "medium",
  )}${field("Estimate reasoning", "estimate_reason", "")}${field("Labels (comma separated)", "tags", t.tags.join(", "))}</div><details><summary>Dependencies, routing & execution limits</summary><div class="form-grid">${textarea("Constraints", "constraints", t.constraints)}<div class="field full"><label>Dependencies (Ctrl/Cmd to select multiple)</label><select name="dependencies" multiple size="4">${data.tasks
    .filter((x) => x.id !== t.id && !x.archived)
    .map(
      (x) =>
        `<option value="${x.id}" ${t.dependencies.includes(x.id) ? "selected" : ""}>${x.key} · ${esc(x.title)}</option>`,
    )
    .join(
      "",
    )}</select></div><div class="field full"><label>Execution directory</label><input name="workspace" value="${esc(t.workspace)}" placeholder="Absolute path, or leave empty for an isolated run directory"><small>CLI agents run here. Concurrent runs in the same directory are serialized.</small></div>${stages
    .slice(1, -1)
    .map(
      (stage) =>
        `<div class="field"><label>${stageNames[stage]} agent override</label>${routingSelect(stage, t.assignments[stage])}</div>`,
    )
    .join(
      "",
    )}${field("Attempts per stage visit (0 = unlimited)", "max_attempts", t.max_attempts, "number", 'min="0" required')}${field("Maximum sendbacks (0 = unlimited)", "max_sendbacks", t.max_sendbacks, "number", 'min="0" required')}${field("Timeout in seconds (0 = unlimited)", "timeout_seconds", t.timeout_seconds, "number", 'min="0" required')}${field("Reported token budget", "token_budget", t.token_budget ?? "", "number", 'min="1"')}</div></details>`;
  $("#overlay").innerHTML = modal(
    existing ? "Edit task" : "Create " + kind,
    body,
    "task-form",
    existing ? "Save changes" : "Create " + kind,
  );
  $("#task-title").focus();
}
function openAgentForm(existing = null) {
  ui.dialog = "agent";
  ui.dialogData = existing;
  const a = existing || {
    name: "",
    adapter: "cli",
    model: "",
    command: "",
    args: [],
    endpoint: "http://127.0.0.1:11434",
    stages: ["planning"],
    instructions: "",
    capacity: 1,
    timeout_seconds: 0,
    enabled: true,
  };
  const body = `<div class="form-grid">${field("Agent name", "name", a.name, "text", 'required')}${selectField(
    "Adapter",
    "adapter",
    [
      ["cli", "Command agent · local or cloud"],
      ["ollama", "Local Ollama · text artifacts"],
    ],
    a.adapter,
  )}${field("Model label", "model", a.model)}${field("Concurrent runs", "capacity", a.capacity, "number", 'min="1" required')}<div class="field full"><label>Stages</label><div class="checks">${stages
    .slice(1, -1)
    .map(
      (s) =>
        `<label class="check-row"><input name="stages" type="checkbox" value="${s}" ${a.stages.includes(s) ? "checked" : ""}>${stageNames[s]}</label>`,
    )
    .join(
      "",
    )}</div></div><div class="field full adapter-cli"><label>Executable</label><input name="command" value="${esc(a.command)}" placeholder="Executable name or absolute path"><small>Runs directly without a shell. The agent receives the handoff JSON on stdin.</small></div><div class="field full adapter-cli"><label>Arguments (JSON array)</label><textarea name="args" spellcheck="false">${esc(JSON.stringify(a.args, null, 2))}</textarea><small>Supported placeholders: {input}, {output}, {workspace}, {prompt_file}, {prompt}. Return result JSON on stdout or write it to {output}.</small></div><div class="field full adapter-cli"><label>Remove inherited environment variables</label><textarea name="unset_env" placeholder="One variable name per line">${esc((a.unset_env || []).join("\n"))}</textarea></div><div class="field full adapter-ollama"><label>Local Ollama URL</label><input name="endpoint" value="${esc(a.endpoint)}"><small>Text-only adapter. For filesystem work or commands, use a CLI agent.</small></div>${textarea("Role instructions", "instructions", a.instructions)}${field("Timeout in seconds (0 = unlimited)", "timeout_seconds", a.timeout_seconds, "number", 'min="0" required')}<label class="check-row"><input type="checkbox" name="enabled" ${a.enabled ? "checked" : ""}> Enabled</label></div>`;
  $("#overlay").innerHTML = modal(
    existing ? "Configure agent" : "Add an agent",
    body,
    "agent-form",
    existing ? "Save agent" : "Add agent",
  );
  adapterVisibility();
  $("#f-name").focus();
}
function adapterVisibility() {
  const val = $('[name="adapter"]')?.value;
  document
    .querySelectorAll(".adapter-cli")
    .forEach((e) => (e.hidden = val !== "cli"));
  document
    .querySelectorAll(".adapter-ollama")
    .forEach((e) => (e.hidden = val !== "ollama"));
}
function openActionDialog(action, destination = "") {
  const t = data.tasks.find((t) => t.id === ui.selected);
  if (!t) return;
  ui.dialog = "action";
  ui.dialogData = { action };
  if (["approve", "reopen_approval"].includes(action)) {
    const title = action === "approve" ? "Record approval" : "Reopen approval";
    const body = `<div class="form-grid">${textarea("Evidence or reason", "reason")}${field("Operator approval code", "approval_code", "", "password", 'required autocomplete="off"')}<small>Use the code supplied at Relay startup or printed in its startup terminal.</small></div>`;
    $("#overlay").innerHTML = modal(title, body, "action-form", title);
    $('[name="reason"]').required = true;
    $('[name="reason"]').focus();
    return;
  }
  const send = action === "send_back";
  const title = send
    ? "Send work back"
    : action === "complete_stage"
      ? "Record completion evidence"
      : "Archive task";
  const choices = stages.slice(1, stages.indexOf(t.stage));
  const body = `${send ? `<div class="notice info">Prior artifacts from the destination stage onward will be marked superseded. History is preserved.</div>` : action === "complete_stage" ? '<div class="notice info">Record the deliverable or checks you performed. This becomes a versioned artifact and advances the task.</div>' : '<p class="muted">This stops active work and hides the task from the board. Its history remains in the database and export.</p>'}<div class="form-grid">${
    send
      ? selectField(
          "Return to",
          "stage",
          choices.map((s) => [s, stageNames[s]]),
          destination || choices.at(-1),
        )
      : ""
  }${textarea(send ? "What needs to change?" : action === "complete_stage" ? "Completion evidence" : "Reason", "reason", "")}</div>`;
  $("#overlay").innerHTML = modal(
    title,
    body,
    "action-form",
    send
      ? "Send back"
      : action === "complete_stage"
        ? "Complete stage"
        : "Archive",
  );
  $('[name="reason"]').required = true;
  $('[name="reason"]').focus();
}
function closeModal() {
  ui.dialog = null;
  ui.dialogData = null;
  if (ui.selected) renderDrawer();
  else $("#overlay").innerHTML = "";
}
async function doAction(action, extra = {}, approvalCode = "") {
  const t = data.tasks.find((t) => t.id === ui.selected);
  await api(
    `/tasks/${t.id}/action`,
    "POST",
    {
      action,
      revision: t.revision,
      ...extra,
    },
    approvalCode,
  );
  await refresh(true);
}

document.addEventListener("click", async (e) => {
  const el = e.target.closest(
    "[data-action],[data-page],[data-task],[data-tab],[data-view]",
  );
  if (!el) {
    if (e.target.dataset.backdrop) {
      if (ui.dialog) closeModal();
      else {
        ui.selected = null;
        $("#overlay").innerHTML = "";
      }
    }
    return;
  }
  if (el.dataset.page) {
    navigate(el.dataset.page);
    return;
  }
  if (el.dataset.task) {
    openTask(el.dataset.task);
    return;
  }
  if (el.dataset.tab) {
    ui.tab = el.dataset.tab;
    renderDrawer();
    return;
  }
  if (el.dataset.view) {
    ui.view = el.dataset.view;
    render();
    return;
  }
  const action = el.dataset.action;
  try {
    if (action === "settings-section") {
      navigate("settings", el.dataset.section);
    } else if (action === "reload-settings") {
      ui.settingsDirty = false;
      document.activeElement?.blur();
      await refresh(true);
    } else if (action === "retry-load") {
      await refresh(true);
    } else if (action === "new-workspace") {
      openWorkspaceForm();
    } else if (action === "toggle-maintenance") {
      serverSettings = await api("/global/settings", "PATCH", { draining: !serverSettings.draining });
      await refresh(true);
      toast(serverSettings.draining ? "Maintenance enabled for all workspaces." : "Admissions resumed. Workspace pause preferences are preserved.");
    } else if (action === "toggle-theme" || action === "set-theme") {
      setTheme(action === "set-theme" ? el.dataset.theme : theme === "dark" ? "light" : "dark");
      if (!editingSettings()) render();
      else { el.textContent = theme === "dark" ? "☀" : "☾"; el.setAttribute("aria-label", theme === "dark" ? "Switch to light mode" : "Switch to dark mode"); }
    } else if (action === "board-stage") {
      ui.boardStage = el.dataset.stage;
      render();
    } else if (action === "queue-open") openTask(el.dataset.id);
    else if (action === "new-task") openTaskForm();
    else if (action === "new-epic") openTaskForm(null, "epic");
    else if (action === "new-child") {
      const p = data.tasks.find((t) => t.id === el.dataset.parent);
      openTaskForm(null, p.kind === "epic" ? "task" : "subtask", p.id);
    } else if (action === "edit-task")
      openTaskForm(data.tasks.find((t) => t.id === ui.selected));
    else if (action === "new-agent") openAgentForm();
    else if (action === "edit-agent")
      openAgentForm(data.agents.find((a) => a.id === el.dataset.id));
    else if (action === "close-modal") closeModal();
    else if (action === "close-overlay") {
      ui.selected = null;
      ui.dialog = null;
      $("#overlay").innerHTML = "";
    } else if (action === "toggle-scheduler") {
      await api("/settings", "PATCH", {
        expected_revision: data.revision,
        paused: !data.settings.paused,
      });
      await refresh(true);
      toast(
        data.settings.paused
          ? "Scheduler paused. Active attempts continue."
          : "Scheduler started. Eligible tasks will run.",
      );
    } else if (action === "start-task") await doAction("start");
    else if (action === "pause-task") await doAction("pause");
    else if (action === "resume-task")
      await doAction(
        data.tasks.find((t) => t.id === ui.selected).status === "failed"
          ? "retry"
          : "resume",
      );
    else if (action === "run-task") {
      const t = data.tasks.find((t) => t.id === ui.selected);
      await api(`/tasks/${t.id}/run`, "POST", { revision: t.revision });
      await refresh(true);
    } else if (action === "send-back") openActionDialog("send_back");
    else if (action === "complete-stage") openActionDialog("complete_stage");
    else if (action === "approve-gate") openActionDialog("approve");
    else if (action === "reopen-approval") openActionDialog("reopen_approval");
    else if (action === "archive-task") openActionDialog("archive");
    else if (action === "backup") {
      const r = await api("/backup", "POST", {});
      toast("Backup created: " + r.path);
    } else if (action === "restore-task") {
      const t = data.tasks.find((t) => t.id === el.dataset.id);
      await api(`/tasks/${t.id}/action`, "POST", {
        action: "restore",
        revision: t.revision,
      });
      await refresh(true);
      toast("Task restored");
    } else if (action === "download-artifact") {
      const a = data.artifacts.find((a) => a.id === el.dataset.id);
      const url = URL.createObjectURL(
        new Blob([a.content], { type: "text/plain" }),
      );
      const link = document.createElement("a");
      link.href = url;
      link.download = a.name.replace(/[\\/:*?"<>|]/g, "_");
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    }
  } catch (error) {
    toast(error.message);
    await refresh();
  }
});
document.addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = e.target;
  const f = new FormData(form);
  const submit =
    form.querySelector('[type="submit"]') ||
    form.querySelector(".form-actions button:last-child");
  if (submit) submit.disabled = true;
  const errorEl = form.querySelector(".form-error");
  if (errorEl) errorEl.textContent = "";
  try {
    if (form.id === "task-form") {
      const body = Object.fromEntries(f);
      body.kind = ui.dialogData?.kind || body.kind;
      if (!ui.dialogData) body.human_approval = f.has("human_approval");
      for (const key of [
        "priority",
        "complexity",
        "max_attempts",
        "max_sendbacks",
        "timeout_seconds",
      ])
        body[key] = Number(body[key]);
      body.token_budget = body.token_budget ? Number(body.token_budget) : null;
      body.parent_id = body.parent_id || null;
      body.dependencies = f.getAll("dependencies");
      body.tags = body.tags
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean);
      body.assignments = {};
      for (const s of stages.slice(1, -1)) {
        if (body["route_" + s]) body.assignments[s] = body["route_" + s];
        delete body["route_" + s];
      }
      let task;
      if (ui.dialogData) {
        body.revision = ui.dialogData.revision;
        task = await api("/tasks/" + ui.dialogData.id, "PATCH", body);
      } else task = await api("/tasks", "POST", body);
      ui.dialog = null;
      await refresh(true);
      openTask(task.id);
      toast("Task saved");
    } else if (form.id === "agent-form") {
      const body = Object.fromEntries(f);
      try {
        body.args = body.adapter === "cli" ? JSON.parse(body.args || "[]") : [];
      } catch {
        throw new Error(
          'Arguments must be a JSON array, such as ["--input", "{input}"].',
        );
      }
      if (
        !Array.isArray(body.args) ||
        body.args.some((a) => typeof a !== "string")
      )
        throw new Error("Every argument must be a string.");
      body.unset_env = String(body.unset_env || "")
        .split(/\r?\n/)
        .map((name) => name.trim())
        .filter(Boolean);
      body.stages = f.getAll("stages");
      body.enabled = f.has("enabled");
      body.capacity = Number(body.capacity);
      body.timeout_seconds = Number(body.timeout_seconds);
      await api(
        "/agents" + (ui.dialogData ? "/" + ui.dialogData.id : ""),
        ui.dialogData ? "PATCH" : "POST",
        body,
      );
      ui.dialog = null;
      ui.selected = null;
      $("#overlay").innerHTML = "";
      ui.page = "agents";
      await refresh(true);
      toast("Agent saved");
    } else if (form.id === "workflow-form") {
      const stage_agents = {};
      for (const s of stages.slice(1, -1)) {
        if (f.get("route_" + s)) stage_agents[s] = f.get("route_" + s);
      }
      await api("/settings", "PATCH", {
        expected_revision: Number(form.dataset.revision),
        stage_agents,
        concurrency: Number(f.get("concurrency")),
        independent_review: f.has("independent_review"),
        auto_advance: f.has("auto_advance"),
        retry_delay_seconds: Number(f.get("retry_delay_seconds")),
      });
      ui.settingsDirty = false;
      document.activeElement?.blur();
      await refresh(true);
      toast("Workflow saved for " + workspaceName());
    } else if (form.id === "project-form") {
      const project = await api("/projects", "POST", { name: f.get("name") });
      location.assign(pageUrl("board", project.id));
    } else if (form.id === "settings-form") {
      await api("/settings", "PATCH", {
        expected_revision: Number(form.dataset.revision),
        project_name: f.get("project_name"),
      });
      ui.settingsDirty = false;
      document.activeElement?.blur();
      await refresh(true);
      toast("Settings saved for " + workspaceName());
    } else if (form.id === "action-form") {
      await doAction(
        ui.dialogData.action,
        {
          reason: f.get("reason"),
          stage: f.get("stage"),
        },
        f.get("approval_code") || "",
      );
      ui.dialog = null;
      renderDrawer();
    } else if (form.id === "comment-form") {
      await doAction("comment", { reason: f.get("reason") });
      renderDrawer();
      toast("Context added");
    }
  } catch (error) {
    if (errorEl) errorEl.textContent = error.message;
    else toast(error.message);
  } finally {
    if (submit) submit.disabled = false;
  }
});
document.addEventListener("input", (e) => {
  if (e.target.closest("#settings-form,#workflow-form")) ui.settingsDirty = true;
  if (e.target.id === "queue-search") {
    ui.queueSearch = e.target.value;
    const rows = data.tasks.filter(
      (t) =>
        !t.archived &&
        t.kind !== "epic" &&
        t.stage !== "done" &&
        (
          t.key +
          " " +
          t.title +
          " " +
          t.blocked_reason +
          " " +
          (data.queue_reasons[t.id] || "")
        )
          .toLowerCase()
          .includes(ui.queueSearch.toLowerCase()),
    );
    $("#queue-table").innerHTML = queueTable(rows);
  }
  if (e.target.id === "task-search") {
    ui.search = e.target.value;
    $("#task-view").innerHTML =
      ui.view === "board"
        ? kanban(filteredTasks())
        : taskTable(filteredTasks());
  }
});
function openWorkspaceForm() {
  ui.dialog = "project";
  $("#overlay").innerHTML = modal("New workspace", field("Workspace name", "name", "", "text", "required"), "project-form", "Create workspace");
}
document.addEventListener("change", (e) => {
  if (e.target.closest("#settings-form,#workflow-form")) ui.settingsDirty = true;
  if (e.target.id.startsWith("project-switch")) {
    if (e.target.value === "__add") openWorkspaceForm();
    else location.assign(e.target.value);
    return;
  }
  const map = {
    "type-filter": "type",
    "epic-filter": "epic",
    "status-filter": "status",
  };
  if (map[e.target.id]) {
    ui[map[e.target.id]] = e.target.value;
    render();
  }
  if (e.target.name === "adapter") adapterVisibility();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    if (ui.dialog) closeModal();
    else {
      ui.selected = null;
      $("#overlay").innerHTML = "";
    }
  }
  if (e.key === "Tab" && $('#overlay [role="dialog"]')) {
    const items = [
      ...$('#overlay [role="dialog"]').querySelectorAll(
        "button,input,textarea,select,a[href]",
      ),
    ].filter((e) => !e.disabled && e.offsetParent !== null);
    const first = items[0],
      last = items.at(-1);
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last?.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first?.focus();
    }
  }
  if (
    e.target.matches("input,textarea,select") ||
    e.ctrlKey ||
    e.metaKey ||
    e.altKey
  )
    return;
  if (!globalScope && !ui.error && e.key.toLowerCase() === "n") {
    e.preventDefault();
    openTaskForm();
  }
  if (e.key === "/") {
    e.preventDefault();
    $("#task-search")?.focus();
  }
  if ((e.key === "Enter" || e.key === " ") && e.target.dataset.task) {
    e.preventDefault();
    openTask(e.target.dataset.task);
  }
});
document.addEventListener("dragstart", (e) => {
  const card = e.target.closest("[data-task]");
  if (card) {
    e.dataTransfer.setData("text/plain", card.dataset.task);
    e.dataTransfer.effectAllowed = "move";
  }
});
document.addEventListener("dragover", (e) => {
  const col = e.target.closest("[data-drop-stage]");
  if (col) {
    e.preventDefault();
    col.classList.add("drag-over");
  }
});
document.addEventListener("dragleave", (e) => {
  const col = e.target.closest("[data-drop-stage]");
  if (col && !col.contains(e.relatedTarget)) col.classList.remove("drag-over");
});
document.addEventListener("drop", async (e) => {
  const col = e.target.closest("[data-drop-stage]");
  if (!col) return;
  e.preventDefault();
  col.classList.remove("drag-over");
  const t = data.tasks.find(
    (t) => t.id === e.dataTransfer.getData("text/plain"),
  );
  if (!t) return;
  const target = col.dataset.dropStage;
  if (target === t.stage) return;
  openTask(t.id);
  if (
    stages.indexOf(target) < stages.indexOf(t.stage) &&
    target !== "backlog"
  ) {
    openActionDialog("send_back", target);
  } else if (t.stage === "backlog" && target === "planning") {
    try {
      await doAction("start");
    } catch (e) {
      toast(e.message);
    }
  } else if (stages.indexOf(target) === stages.indexOf(t.stage) + 1) {
    openActionDialog("complete_stage");
  } else
    toast(
      "Complete stages in order, or send work back to an earlier executable stage.",
    );
});
render();
try {
  boot = await api("/bootstrap");
  await refresh(true);
  setInterval(() => refresh(), 1800);
} catch (e) {
  $("#app").innerHTML =
    `<div class="page-error"><h1>${e.status === 404 ? "Workspace unavailable" : "Could not open Relay"}</h1><p>${esc(e.message)}</p><p>Check that the local server is running, then reload this page.</p><a class="btn" href="/?scope=global&page=overview">All workspaces</a></div>`;
}
