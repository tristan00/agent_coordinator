import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

// Exercise the actual page controller without running its automatic network loop.
const app = readFileSync(new URL("../../web/app.js", import.meta.url), "utf8").replaceAll("\r\n", "\n");
const source = app.slice(app.indexOf("const $ ="), app.lastIndexOf("render();\ntry {"));
function browser(search, saved = {}) {
  const handlers = {}, calls = [], nodes = new Map();
  const storage = new Map(Object.entries(saved));
  const ctx = vm.createContext({
    URLSearchParams, console, setTimeout: () => 1, clearTimeout() {},
    location: { search, assign(url) { calls.push({ navigate: url }); }, reload() {} },
    history: { pushState(_s, _t, url) { calls.push({ history: url }); } },
    localStorage: { getItem: k => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v) },
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    window: { addEventListener() {} },
    document: {
      documentElement: { dataset: {} }, activeElement: null,
      addEventListener: (name, handler) => { handlers[name] = handler; },
      querySelector(selector) {
        if (!nodes.has(selector)) nodes.set(selector, { innerHTML: "", textContent: "", classList: { add() {}, remove() {} } });
        return nodes.get(selector);
      },
    },
    fetch: async (url, options) => {
      calls.push({ url, options });
      return { ok: true, json: async () => ({ token: "session", revision: 1, tasks: [], agents: [], attempts: [], events: [], artifacts: [], queue_reasons: {}, settings: { project_name: "Alpha", paused: true, stage_agents: {} } }) };
    },
    mountMetrics() {}, unmountMetrics() {},
    coordinationSignature: () => "same",
    deriveWorkerStatus: () => "idle", queueTaskState: () => ({}),
    FormData: class extends Map { constructor(form) { super(Object.entries(form.fields)); } },
  });
  vm.runInContext(source, ctx);
  return { ctx, calls, nodes, handlers, storage, run: code => vm.runInContext(code, ctx) };
}

test("explicit workspace URLs win over stored navigation and normalize legacy Workflow", () => {
  const b = browser("?project=alpha&page=settings&section=data", { "relay-page:alpha": "agents" });
  assert.equal(b.run("ui.page"), "settings");
  assert.equal(b.run("ui.settingsSection"), "data");
  assert.equal(browser("?project=beta&page=workflow").run("ui.settingsSection"), "workflow");
  assert.equal(browser("?project=alpha&page=unknown").run("ui.page"), "board");
  assert.equal(browser("?scope=global&page=unknown").run("ui.page"), "overview");
});

test("navigation preferences belong to each workspace and switch links preserve destination", () => {
  const b = browser("?project=alpha&page=insights");
  b.run('data.projects = [{id:"alpha",name:"Alpha"},{id:"beta",name:"Beta"}]; ui.loading = false; navigate("archive")');
  assert.equal(b.storage.get("relay-page:alpha"), "archive");
  assert.equal(b.storage.has("relay-page:beta"), false);
  assert.match(b.run("projectOptions()"), /project=beta&amp;page=archive/);
  assert.match(b.run("projectOptions()"), /scope=global&amp;page=global-settings/);
  assert.match(b.run('pageUrl("settings", "x&y", "workflow")'), /project=x%26y&page=settings&section=workflow/);
});

test("global bootstrap and maintenance requests are independent of the selected workspace", async () => {
  const b = browser("?scope=global&project=missing&page=global-settings");
  await b.run('api("/bootstrap")');
  await b.run('api("/global/settings", "PATCH", {draining:true})');
  assert.equal(b.calls[0].url, "/api/bootstrap");
  assert.equal(b.calls[1].url, "/api/global/settings");
  assert.deepEqual(JSON.parse(b.calls[1].options.body), { draining: true });
});

test("dirty settings survive polling after blur, and saves use the form's original revision", async () => {
  const b = browser("?project=alpha&page=settings");
  b.run('data.revision = 7; data.settings.project_name = "Alpha"; ui.loading = false; render(); ui.settingsDirty = true;');
  const before = b.nodes.get("#app").innerHTML;
  b.ctx.fetch = async (url, options) => {
    b.calls.push({ url, options });
    return { ok: true, json: async () => ({ revision: 8, tasks: [], agents: [], attempts: [], events: [], artifacts: [], queue_reasons: {}, settings: { project_name: "Remote edit", paused: true, stage_agents: {} } }) };
  };
  await b.run("refresh()");
  assert.equal(b.run("data.revision"), 8);
  assert.equal(b.nodes.get("#app").innerHTML, before);
  assert.match(before, /data-revision="7"/);
  const form = { id: "settings-form", dataset: { revision: "7" }, fields: { project_name: "My draft" }, querySelector: () => null };
  await b.handlers.submit({ preventDefault() {}, target: form });
  const patch = b.calls.find(c => c.options.method === "PATCH");
  assert.deepEqual(JSON.parse(patch.options.body), { expected_revision: 7, project_name: "My draft" });
  assert.equal(patch.url, "/api/projects/alpha/settings");
});

test("usage distinguishes absent values from partial reported totals", () => {
  const b = browser("?scope=global&page=overview");
  const html = b.run('usagePanel({attempts:4,failed_attempts:1,sendbacks:1,duration_ms:2000,input_tokens:123,input_token_attempts:1,output_tokens:null,output_token_attempts:0,cost_usd:null,cost_attempts:0})');
  assert.match(html, /1 \/ 4 attempts reporting · Partial total/);
  assert.match(html, /Unavailable/);
  assert.doesNotMatch(html, /\$0\.0000/);
});

test("workspace settings owns Workflow, while Archive remains a work destination", () => {
  const b = browser("?project=alpha&page=settings");
  b.run('data.settings.project_name = "Alpha <script>"; data.revision = 4;');
  assert.doesNotMatch(b.run("settings()"), /Archived tasks/);
  assert.match(b.run("settings()"), /data-section="workflow"/);
  assert.match(b.run('shell("content")'), /data-page="archive"/);
  assert.doesNotMatch(b.run('shell("content")'), /data-page="workflow"/);
  assert.match(b.run('shell("content")'), /Alpha &lt;script&gt;/);
});
