"use strict";

const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => [...document.querySelectorAll(sel)];
const MAX_LINES = 1000;
const SPARK_POINTS = 90; // stats arrive every 2 s: 3 minutes
const ICONS = "/static/icons.svg";
const FLAVOR_NAME = { vanilla: "Vanilla", paper: "Paper", fabric: "Fabric" };
const PAGES = { overview: "Overview", players: "Players", addons: "Add-ons", world: "World & backups", server: "Server" };
const LEVELS = ["info", "warn", "err", "chat", "gui"];

const ui = {
  sid: null,          // selected server id (see instances.py)
  page: "overview",
  servers: [],        // [{ id, name, state, port, flavor, version, has_map, ... }]
  server: null,       // { id, name, port, has_map } of the selected server
  status: null,
  settings: null,     // saved settings of the selected server
  world: {},          // { version, flavor, rank } that last ran the world
  installed: [],      // [{ flavor, version, build, size_mb, rank }] (shared by all servers)
  sel: null,          // unsaved selection { flavor, version }
  versions: [],       // version list for the selected flavor
  latest: null,
  access: null,
  datapacks: null,
  addons: null,       // { kind: "mods" | "plugins" | null, version, running, items }
  addonUpdates: null, // { file: { version } } from the last update check
  atab: store("panel.addonTab") || "datapacks",
  history: [],
  historyPos: 0,
  ptab: "online",
  spark: { cpu: [], ram: [] },
  logCounts: {},
  ws: null,
  wsRetry: 1000,
};

/** URL of a per-server API endpoint. */
const S = (path = "") => `/api/servers/${encodeURIComponent(ui.sid)}${path}`;

// ---------- helpers ----------

async function api(path, { method = "GET", body } = {}) {
  const res = await fetch(path, {
    method,
    headers: body ? { "Content-Type": "application/json" } : {},
    body: body ? JSON.stringify(body) : undefined,
    credentials: "same-origin",
  });
  if (res.status === 401 && path !== "/api/login") {
    showLogin();
    throw new Error("Not logged in");
  }
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.detail || `HTTP ${res.status}`);
  return data;
}

let toastTimer;
function toast(msg, isError = false) {
  const t = $("#toast");
  t.textContent = msg;
  t.className = "toast" + (isError ? " err" : "");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.add("hidden"), 4000);
}

function fmtDuration(sec) {
  if (!sec) return "";
  const h = Math.floor(sec / 3600), m = Math.floor((sec % 3600) / 60);
  return h ? `up ${h} h ${m} min` : `up ${m} min`;
}

function fmtMB(mb) {
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${mb} MB`;
}

function el(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === "dataset") Object.assign(node.dataset, v);
    else if (k.startsWith("aria") || k === "role") node.setAttribute(k.replace(/^aria/, "aria-").toLowerCase(), v);
    else node[k] = v;
  }
  node.append(...children.filter((c) => c != null));
  return node;
}

function icon(name, cls = "i") {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", cls);
  const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
  use.setAttribute("href", `${ICONS}#ph-${name}`);
  svg.append(use);
  return svg;
}

/** Panel-styled replacement for confirm() / prompt(): resolves true when the action button is used
 *  (Escape, Cancel or a click on the backdrop = false). With `input` ({label, placeholder, value,
 *  maxLength, match}) it shows a text field and resolves to its text, or null when cancelled;
 *  `match` keeps the action button disabled until the field equals it (typed confirmation). */
let askDialog = null;
function ask({ title, text = "", ok = "OK", danger = false, iconName = null, input = null }) {
  if (!askDialog) {
    askDialog = el("dialog", { className: "dialog ask-dialog" });
    askDialog.addEventListener("click", (e) => { if (e.target === askDialog) askDialog.close(""); });
    document.body.append(askDialog);
  }
  const cancel = el("button", { type: "button", className: "btn ghost", textContent: "Cancel" });
  const go = el("button", { type: "button", className: `btn ${danger ? "danger" : "primary"}` },
    iconName ? icon(iconName) : null, ok);
  let field = null;
  if (input) {
    field = el("input", { type: "text", autocomplete: "off", value: input.value || "",
      placeholder: input.placeholder || "", maxLength: input.maxLength || 200 });
    if (input.match != null) {
      const check = () => { go.disabled = field.value.trim() !== input.match; };
      field.addEventListener("input", check);
      check();
    }
  }
  cancel.addEventListener("click", () => askDialog.close(""));
  const form = el("form", { method: "dialog" },
    el("div", { className: "card-head" }, el("h2", { textContent: title })),
    ...text.split("\n").filter(Boolean).map((line, i) => el("p", { className: i ? "muted" : "", textContent: line })),
    field ? el("label", { className: "field" }, input.label ? el("span", { className: "label", textContent: input.label }) : null, field) : null,
    el("div", { className: "dialog-actions" }, el("span"), cancel, go));
  // Enter in the field and the action button both submit
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    if (!go.disabled) askDialog.close("ok");
  });
  go.type = "submit";
  askDialog.replaceChildren(form);
  askDialog.returnValue = "";
  askDialog.showModal();
  (field || go).focus();
  return new Promise((resolve) => askDialog.addEventListener("close", () => {
    const yes = askDialog.returnValue === "ok";
    resolve(field ? (yes ? field.value.trim() : null) : yes);
  }, { once: true }));
}

function tag(text, cls = "", iconName = null) {
  return el("span", { className: `tag ${cls}` }, iconName ? icon(iconName) : null, text);
}

const flavorLabel = (f, v) => `${FLAVOR_NAME[f] || f} ${v}`;

/** Stable pastel colour per name (shown until / instead of the skin head). */
function nameColor(name) {
  let h = 0;
  for (const c of name.toLowerCase()) h = (h * 31 + c.charCodeAt(0)) % 360;
  return `hsl(${h} 62% 72%)`;
}

// Skin heads: the panel hands out the skin PNG (from Mojang, cached), the face is cut out here
const heads = new Map(); // "<server>/<name>" -> Promise<data URL | null>

function headUrl(name) {
  if (!/^\w{3,16}$/.test(name)) return Promise.resolve(null);
  const key = `${ui.sid}/${name.toLowerCase()}`;
  if (!heads.has(key)) {
    heads.set(key, (async () => {
      const res = await fetch(S(`/players/${encodeURIComponent(name)}/skin`), { credentials: "same-origin" });
      if (!res.ok) return null;
      const skin = await createImageBitmap(await res.blob());
      const c = document.createElement("canvas");
      c.width = c.height = 8;
      const g = c.getContext("2d", { willReadFrequently: true });
      // Hat layer first, alone: old 64×32 skins often fill it opaque (Minecraft ignores it then)
      g.drawImage(skin, 40, 8, 8, 8, 0, 0, 8, 8);
      const hat = g.getImageData(0, 0, 8, 8);
      const opaque = hat.data.every((v, i) => i % 4 !== 3 || v === 255);
      g.clearRect(0, 0, 8, 8);
      g.drawImage(skin, 8, 8, 8, 8, 0, 0, 8, 8);    // face
      if (!(skin.height === 32 && opaque)) g.drawImage(skin, 40, 8, 8, 8, 0, 0, 8, 8);
      return c.toDataURL();
    })().catch(() => null));
  }
  return heads.get(key);
}

function avatar(name, small = false, big = false) {
  const a = el("span", { className: "avatar" + (small ? " small" : "") + (big ? " big" : ""), textContent: name[0].toUpperCase(), title: name });
  a.style.background = nameColor(name);
  a.setAttribute("aria-hidden", "true");
  headUrl(name).then((url) => {
    if (!url) return;
    a.textContent = "";
    a.classList.add("head");
    a.style.background = `center / cover no-repeat url(${url})`;
  });
  return a;
}

/** A player name that opens the profile. */
function playerName(name, cls = "title") {
  const b = el("button", { type: "button", className: `link-btn ${cls}`, textContent: name, title: `Profile of ${name}` });
  b.addEventListener("click", () => openProfile(name));
  return b;
}

function switchInput(checked, label, onChange, disabled = false) {
  const input = el("input", { type: "checkbox", checked, disabled, role: "switch", ariaLabel: label });
  input.addEventListener("change", () => onChange(input));
  return el("label", { className: "switch", title: label }, input, el("span", { className: "track" }));
}

function store(key, value) {
  try {
    if (value === undefined) return JSON.parse(localStorage.getItem(key));
    localStorage.setItem(key, JSON.stringify(value));
  } catch { /* storage blocked: only a convenience */ }
  return null;
}

// ---------- login ----------

function showLogin() {
  $("#app").classList.add("hidden");
  $("#login").classList.remove("hidden");
  $("#password").focus();
  if (ui.ws) { const ws = ui.ws; ui.ws = null; ws.close(); }
}

$("#login-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  $("#login-error").textContent = "";
  try {
    await api("/api/login", { method: "POST", body: { password: $("#password").value } });
    $("#password").value = "";
    await boot();
  } catch (err) {
    $("#login-error").textContent = err.message;
  }
});

$("#logout").addEventListener("click", async () => {
  await api("/api/logout", { method: "POST" }).catch(() => {});
  showLogin();
});

// ---------- routing: #/<server>/<page> ----------

function parseHash() {
  const [, sid, page] = location.hash.match(/^#\/([a-z0-9-]+)(?:\/(\w+))?/) || [];
  return { sid, page: PAGES[page] ? page : "overview" };
}

function go(sid, page = ui.page) {
  location.hash = `#/${sid}/${page}`;
}

async function route() {
  let { sid, page } = parseHash();
  if (!ui.servers.some((s) => s.id === sid)) sid = ui.servers.some((s) => s.id === ui.sid) ? ui.sid : "main";
  const changed = sid !== ui.sid;
  ui.page = page;
  if (changed) await switchServer(sid);
  showPage();
  if (location.hash !== `#/${sid}/${page}`) history.replaceState(null, "", `#/${sid}/${page}`);
}

window.addEventListener("hashchange", () => route().catch((err) => toast(err.message, true)));

function showPage() {
  for (const p of $$(".page")) p.classList.toggle("hidden", p.dataset.page !== ui.page);
  for (const a of $$(".nav a[data-page]")) {
    a.href = `#/${ui.sid}/${a.dataset.page}`;
    if (a.dataset.page === ui.page) a.setAttribute("aria-current", "page");
    else a.removeAttribute("aria-current");
  }
  $("#page-title").textContent = PAGES[ui.page];
  document.title = `${ui.server?.name || "Server"} · ${PAGES[ui.page]} · Server Panel`;
  if (ui.page === "players") loadAccess();
  if (ui.page === "server") showServerIcon();
  if (ui.page === "world") {
    loadCaveReach();
    loadWaypoints();
  }
  if (ui.page === "addons") showAddonTab(ui.atab);
}

async function switchServer(sid) {
  ui.sid = sid;
  ui.server = null;
  ui.status = null;
  ui.sel = null;
  ui.versions = [];
  ui.access = null;
  ui.datapacks = null;
  ui.addons = null;
  ui.addonUpdates = null;
  $("#mr-results").replaceChildren();
  ui.spark = { cpu: [], ram: [] };
  $("#history-card").classList.add("hidden");
  closeProfile(); // the profile belongs to the previous server
  ui.history = [];
  ui.historyPos = 0;
  renderServerList();
  drawSpark("#spark-cpu", [], 100);
  drawSpark("#spark-ram", [], 1);
  await loadState(true);
}

// ---------- server list (sidebar) ----------

async function loadServers() {
  ui.servers = await api("/api/servers");
  renderServerList();
}

function renderServerList() {
  const ul = $("#server-list");
  ul.replaceChildren(...ui.servers.map((s) => {
    const a = el("a", {
      className: "server-item", href: `#/${s.id}/${ui.page}`,
      title: `${s.name} — ${s.state}${s.port ? ` · port ${s.port}` : ""}`,
    },
    el("span", { className: `server-icon ${s.flavor}`, textContent: s.name[0].toUpperCase() }),
    el("span", { className: "server-text" },
      el("b", { textContent: s.name }),
      el("span", { textContent: [s.version ? flavorLabel(s.flavor, s.version) : FLAVOR_NAME[s.flavor], `:${s.port}`].join(" · ") })),
    el("span", { className: `dot-state ${s.state}`, title: s.state }));
    if (s.id === ui.sid) a.setAttribute("aria-current", "true");
    return el("li", {}, a);
  }));
  $("#server-count").textContent = ui.servers.length === 1 ? "1 server" : `${ui.servers.length} servers`;
}

/** Status event of any server: keep its dot in the sidebar current. */
function updateServerEntry(status) {
  const s = ui.servers.find((x) => x.id === status.server);
  if (!s) return;
  const players = status.players.length;
  if (s.state === status.state && s.task === status.task && s.players === players) return;
  Object.assign(s, { state: status.state, task: status.task, players });
  renderServerList();
}

// ---------- new server ----------

const serverDialog = $("#server-dialog");

async function loadNewServerVersions() {
  const flavor = $('input[name="ns-flavor"]:checked').value;
  const select = $("#ns-version");
  select.replaceChildren(el("option", { value: "", textContent: "Loading…" }));
  try {
    const data = await api(`/api/versions?flavor=${flavor}&snapshots=false`);
    select.replaceChildren(...data.versions.map((v) => el("option", {
      value: v.id, textContent: v.id + (v.id === data.latest ? " (latest)" : ""), selected: v.id === data.latest,
    })));
  } catch (err) {
    // Offline: offer what is already downloaded (or choose later on the Server page)
    const local = ui.installed.filter((i) => i.flavor === flavor);
    select.replaceChildren(el("option", { value: "", textContent: "Choose later" }),
      ...local.map((i) => el("option", { value: i.version, textContent: i.version })));
    toast(err.message, true);
  }
}

$("#new-server-btn").addEventListener("click", () => {
  $("#ns-name").value = "";
  serverDialog.showModal();
  loadNewServerVersions();
});
$$('input[name="ns-flavor"]').forEach((r) => r.addEventListener("change", loadNewServerVersions));
serverDialog.querySelectorAll("[data-close]").forEach((b) => b.addEventListener("click", () => serverDialog.close()));

$("#server-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  try {
    const created = await api("/api/servers", {
      method: "POST",
      body: {
        name: $("#ns-name").value, flavor: $('input[name="ns-flavor"]:checked').value,
        version: $("#ns-version").value, min_ram: $("#ns-min").value, max_ram: $("#ns-max").value,
      },
    });
    serverDialog.close();
    await loadServers();
    toast(`${created.name} created (game port ${created.port})`);
    go(created.id, "server");
  } catch (err) {
    toast(err.message, true);
  }
});

$("#rename-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  try {
    const s = await api(S(), { method: "PATCH", body: { name: $("#server-rename").value } });
    ui.server.name = s.name;
    await loadServers();
    renderServerInfo();
    toast("Renamed");
  } catch (err) {
    toast(err.message, true);
  }
});

$("#port-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const port = Number($("#server-port").value);
  if (!ui.server || port === ui.server.port) return;
  if (!(await ask({ title: `Change the game port to ${port}?`,
                    text: `Players then connect with <address>:${port}.\nOpen the port in your router (and in Docker) for friends outside your network.`,
                    ok: "Change port", iconName: "floppy-disk" }))) {
    $("#server-port").value = ui.server.port;
    return;
  }
  try {
    const s = await api(S("/port"), { method: "PUT", body: { port } });
    ui.server.port = s.port;
    await loadServers();
    renderServerInfo();
    toast(`Game port set to ${s.port}`);
  } catch (err) {
    toast(err.message, true);
  }
});

$("#delete-server-btn").addEventListener("click", async () => {
  const name = ui.server?.name;
  const typed = await ask({ title: `Delete the server "${name}"?`, text: "Its world and settings are deleted. Its backups are kept.",
    ok: "Delete server", danger: true, iconName: "trash", input: { label: "Type the server name to confirm", match: name } });
  if (typed === null) return;
  try {
    await api(S(), { method: "DELETE" });
    await loadServers();
    toast(`${name} deleted`);
    go("main", "overview");
  } catch (err) {
    toast(err.message, true);
  }
});

function renderServerInfo() {
  const s = ui.server;
  if (!s) return;
  $("#server-name").textContent = s.name;
  $("#server-rename").value = s.name;
  $("#port-label").textContent = `port ${s.port}`;
  if (document.activeElement !== $("#server-port")) $("#server-port").value = s.port;
  $("#fact-id").textContent = s.id === "main" ? "data/server" : `data/servers/${s.id}`;
  $("#delete-server-btn").classList.toggle("hidden", s.id === "main");
  // every server has its own live map (has_map stays as a flag for the API)
  $("#map-box").classList.toggle("hidden", !s.has_map);
  $("#map-note").textContent = s.has_map ? "" : "The live map is only available for the first server (for now).";
  $("#world-map-note").textContent = (s.has_map ? "The live map is reset. " : "") + "The new world is generated on the next start.";
  document.title = `${s.name} · ${PAGES[ui.page]} · Server Panel`;
}

// ---------- console & chat ----------

function lineClass(line) {
  if (line.startsWith("[Panel]")) return "gui";
  if (/(\/| )(ERROR|FATAL)\]|Exception/.test(line)) return "err";
  if (/(\/| )WARN\]/.test(line)) return "warn";
  if (/\]: (\[Not Secure\] )?<\w+>/.test(line)) return "chat";
  return "info";
}

const logQuery = () => $("#log-search").value.trim().toLowerCase();

function appendLog(lines) {
  const con = $("#console");
  const q = logQuery();
  const frag = document.createDocumentFragment();
  for (const line of lines) {
    const cls = lineClass(line);
    ui.logCounts[cls] = (ui.logCounts[cls] || 0) + 1;
    const span = el("span", { className: cls, textContent: line });
    if (q && !line.toLowerCase().includes(q)) span.classList.add("nomatch");
    frag.append(span);
  }
  con.append(frag);
  while (con.childElementCount > MAX_LINES) {
    const first = con.firstElementChild;
    const cls = LEVELS.find((l) => first.classList.contains(l));
    if (cls) ui.logCounts[cls]--;
    first.remove();
  }
  renderLogCounts();
  if ($("#autoscroll").checked) con.scrollTop = con.scrollHeight;
}

function resetLog(lines) {
  $("#console").replaceChildren();
  ui.logCounts = {};
  appendLog(lines);
}

function renderLogCounts() {
  for (const chip of $$("#log-filters .chip")) chip.querySelector(".n").textContent = ui.logCounts[chip.dataset.level] || 0;
}

function applyLogFilters() {
  const hidden = store("panel.logHidden") || [];
  for (const chip of $$("#log-filters .chip")) {
    const off = hidden.includes(chip.dataset.level);
    chip.setAttribute("aria-pressed", String(!off));
    $("#console").classList.toggle(`hide-${chip.dataset.level}`, off);
  }
}

$$("#log-filters .chip").forEach((chip) => chip.addEventListener("click", () => {
  const hidden = new Set(store("panel.logHidden") || []);
  const level = chip.dataset.level;
  if (hidden.has(level)) hidden.delete(level); else hidden.add(level);
  store("panel.logHidden", [...hidden]);
  applyLogFilters();
}));

$("#log-search").addEventListener("input", () => {
  const q = logQuery();
  for (const span of $("#console").children) {
    span.classList.toggle("nomatch", !!q && !span.textContent.toLowerCase().includes(q));
  }
});

function appendChat(entry) {
  const chat = $("#chat");
  chat.querySelector(".empty")?.remove();
  const m = entry.text.match(/^<(\w{1,16})> (.*)$/);
  let row;
  if (m) {
    row = el("div", { className: "chat-line" }, avatar(m[1], true),
      el("div", { className: "msg" },
        el("span", { className: "who" }, el("b", { textContent: m[1] }), el("time", { textContent: entry.time })),
        el("span", { className: "text", textContent: m[2] })));
  } else {
    row = el("div", { className: "chat-line event" },
      el("span", { className: "text", textContent: entry.text }), el("time", { textContent: entry.time }));
  }
  chat.append(row);
  while (chat.childElementCount > 300) chat.firstElementChild.remove();
  chat.scrollTop = chat.scrollHeight;
}

function resetChat(entries) {
  $("#chat").replaceChildren();
  if (!entries.length) $("#chat").append(el("div", { className: "empty", textContent: "No chat messages yet" }));
  entries.forEach(appendChat);
}

// ---------- status / stats ----------

function renderStatus(s) {
  ui.status = s;
  updateServerEntry(s);
  const pill = $("#state-pill");
  pill.textContent = s.state;
  pill.className = "pill " + s.state;
  $("#version-label").textContent = s.version ? flavorLabel(s.flavor, s.version) : "";
  $("#uptime-label").textContent = fmtDuration(s.uptime);

  const task = $("#task-label");
  task.classList.toggle("hidden", !s.task);
  task.textContent = s.task ? `${s.task} in progress…` : "";

  $("#eula-banner").classList.toggle("hidden", !s.eula_missing);

  // The public map runs on its own port (plain HTTP), same host as the panel
  const mapUrl = s.map_port ? `http://${location.hostname}:${s.map_port}${s.map_path || "/"}` : null;
  ui.mapUrl = mapUrl;
  for (const a of [$("#map-link"), $("#map-open")]) {
    a.classList.toggle("hidden", !mapUrl);
    if (mapUrl) a.href = mapUrl;
  }
  $("#map-render").classList.toggle("hidden", !ui.settings?.map_enabled || !ui.server?.has_map);

  const alive = s.state !== "stopped";
  const busy = !!s.task;
  const btn = (a) => document.querySelector(`[data-action="${a}"]`);
  btn("start").disabled = alive || busy;
  btn("stop").disabled = !alive || busy;
  btn("restart").disabled = !alive || busy;
  btn("backup").disabled = busy;
  btn("kill").classList.toggle("hidden", !alive);
  btn("cancel").classList.toggle("hidden", !["stop", "restart"].includes(s.task));
  $("#install-btn").disabled = busy;

  $("#player-count").textContent = s.players.length;
  $("#nav-players").textContent = s.players.length || "";
  $("#count-online").textContent = s.players.length;
  $("#player-heads").replaceChildren(...s.players.map((n) => {
    const b = el("button", { type: "button", className: "head-btn", title: `Profile of ${n}`, ariaLabel: `Profile of ${n}` }, avatar(n, true));
    b.addEventListener("click", () => openProfile(n));
    return b;
  }));
  const list = $("#players");
  list.replaceChildren();
  if (!s.players.length) {
    list.append(el("li", { className: "empty", textContent: alive ? "Nobody online" : "Server offline" }));
  }
  for (const name of s.players) {
    list.append(el("li", { className: lower(name) === lower(ui.profile?.name || "") ? "selected" : "" }, avatar(name),
      el("div", { className: "grow" }, playerName(name),
        el("div", { className: "meta", textContent: [onWhitelist(name) ? "whitelisted" : null, isOp(name) ? "operator" : null].filter(Boolean).join(" · ") || "online" })),
      playerActions(name)));
  }
  if (ui.profile) renderProfileButtons();
  renderInstalled(); // running marker / delete buttons depend on status
  if (ui.datapacks) renderDatapacks(ui.datapacks);
  renderAccess();    // "recently rejected" comes with the status
}

function drawSpark(sel, values, max, points = SPARK_POINTS) {
  const svg = $(sel);
  const line = svg.querySelector("polyline"), area = svg.querySelector(".area");
  if (values.length < 2) { line.setAttribute("points", ""); area.setAttribute("d", ""); return; }
  const w = 240, h = 38, step = w / (points - 1), x0 = w - step * (values.length - 1);
  const pts = values.map((v, i) => `${(x0 + i * step).toFixed(1)},${(h + 1 - Math.min(v / max, 1) * h).toFixed(1)}`);
  line.setAttribute("points", pts.join(" "));
  area.setAttribute("d", `M${x0.toFixed(1)},40 L${pts.join(" L")} L${w},40 Z`);
}

function pushSpark(key, value) {
  const arr = ui.spark[key];
  arr.push(value);
  if (arr.length > SPARK_POINTS) arr.shift();
}

function renderStats(st) {
  const mc = st.minecraft;
  $("#s-mc-state").textContent = mc ? "% of the machine" : "offline";
  $("#s-mc-cpu").textContent = mc ? `${mc.cpu.toFixed(1)} %` : "–";
  // RAM against the configured -Xmx (the JVM may use a bit more than the heap)
  $("#s-mc-ram").textContent = mc ? fmtMB(mc.ram) : "–";
  $("#s-mc-ram-max").textContent = mc?.ram_max ? `of ${fmtMB(mc.ram_max)}` : "";
  $("#b-mc-ram").style.width = mc?.ram_max ? `${Math.min((mc.ram / mc.ram_max) * 100, 100)}%` : "0";
  if (mc) {
    pushSpark("cpu", mc.cpu);
    pushSpark("ram", mc.ram);
  } else {
    ui.spark = { cpu: [], ram: [] }; // stopped: no curve (a new process starts from zero)
  }
  drawSpark("#spark-cpu", ui.spark.cpu, Math.max(10, ...ui.spark.cpu) * 1.15);
  drawSpark("#spark-ram", ui.spark.ram, mc?.ram_max || Math.max(1, ...ui.spark.ram));
  const t = st.tick;
  $("#s-tps").textContent = t?.tps != null ? t.tps.toFixed(1) : "–";
  $("#s-tps").className = "stat-value" + (t?.tps != null && t.tps < 15 ? " bad" : t?.tps != null && t.tps < 19 ? " meh" : "");
  $("#s-mspt").textContent = t?.mspt != null ? `${t.mspt.toFixed(1)} ms/tick` : mc ? "" : "offline";
  $("#s-panel").textContent = `${st.panel.cpu.toFixed(1)} % · ${fmtMB(st.panel.ram)}`;
  $("#net-row").classList.toggle("hidden", !st.net);
  if (st.net) $("#s-net").textContent = `${st.net.up} / ${st.net.down} KB/s`;
  if (ui.status) $("#uptime-label").textContent = fmtDuration(st.uptime);
}

// ---------- whitelist & operators ----------

const lower = (n) => n.toLowerCase();
const onWhitelist = (n) => (ui.access?.whitelist || []).some((w) => lower(w) === lower(n));
const isOp = (n) => (ui.access?.ops || []).some((o) => lower(o.name) === lower(n));
const banOf = (n) => (ui.access?.bans || []).find((b) => lower(b.name) === lower(n));

/** Quick toggles for a player row: whitelist + operator + kick. */
function playerActions(name) {
  const wl = onWhitelist(name), op = isOp(name);
  const mk = (iconName, on, title, kind) => {
    const b = el("button", { className: `icon-btn small${on ? " on" : ""}`, title, ariaLabel: title }, icon(iconName));
    b.addEventListener("click", () => changeAccess(kind, on ? "remove" : "add", name));
    return b;
  };
  const kick = el("button", { className: "icon-btn small danger", title: `Kick ${name}`, ariaLabel: `Kick ${name}` }, icon("sign-out"));
  kick.addEventListener("click", async () => {
    if (!(await ask({ title: `Kick ${name}?`, text: "They can join again right away.", ok: "Kick", danger: true, iconName: "sign-out" }))) return;
    try {
      await api(S("/command"), { method: "POST", body: { command: `kick ${name}` } });
    } catch (err) {
      toast(err.message, true);
    }
  });
  return el("span", { className: "row-actions" },
    mk("list-checks", wl, wl ? `Remove ${name} from whitelist` : `Add ${name} to whitelist`, "whitelist"),
    mk("crown-simple", op, op ? `Remove operator rights from ${name}` : `Make ${name} operator`, "ops"),
    kick);
}

function removeButton(title, onClick) {
  const b = el("button", { className: "icon-btn small danger", title, ariaLabel: title }, icon("x"));
  b.addEventListener("click", onClick);
  return b;
}

function renderAccess() {
  const a = ui.access;
  if (!a) return;
  $("#wl-enabled").checked = a.whitelist_enabled;
  $("#count-whitelist").textContent = a.whitelist.length;
  $("#count-ops").textContent = a.ops.length;

  const wl = $("#whitelist");
  wl.replaceChildren();
  // Players the whitelist just turned away, with a one-click "add"
  for (const name of (ui.status?.rejected || []).filter((n) => !onWhitelist(n))) {
    const add = el("button", { className: "btn small", title: `Add ${name} to whitelist` }, icon("user-plus"), "Add");
    add.addEventListener("click", () => changeAccess("whitelist", "add", name));
    wl.append(el("li", { className: "current" }, avatar(name),
      el("div", { className: "grow" }, el("div", { className: "title", textContent: name }),
        el("div", { className: "meta", textContent: "just tried to join — not whitelisted" })),
      add));
  }
  if (!a.whitelist.length && !wl.childElementCount) {
    wl.append(el("li", { className: "empty", textContent: "Nobody on the whitelist yet" }));
  }
  for (const name of a.whitelist) {
    wl.append(el("li", {}, avatar(name),
      el("span", { className: "grow" }, playerName(name)),
      removeButton(`Remove ${name} from whitelist`, () => changeAccess("whitelist", "remove", name))));
  }

  const ops = $("#ops");
  ops.replaceChildren();
  if (!a.ops.length) ops.append(el("li", { className: "empty", textContent: "No operators yet" }));
  for (const o of a.ops) {
    ops.append(el("li", {}, avatar(o.name),
      el("div", { className: "grow" }, playerName(o.name),
        el("div", { className: "meta", textContent: `permission level ${o.level}` })),
      removeButton(`Remove operator rights from ${o.name}`, () => changeAccess("ops", "remove", o.name))));
  }

  const bans = $("#bans");
  bans.replaceChildren();
  $("#count-bans").textContent = a.bans.length;
  if (!a.bans.length) bans.append(el("li", { className: "empty", textContent: "Nobody is banned" }));
  for (const b of a.bans) {
    const meta = [b.reason, b.source ? `by ${b.source}` : null, b.created ? b.created.slice(0, 16) : null,
                  b.expires && b.expires !== "forever" ? `until ${b.expires.slice(0, 16)}` : null].filter(Boolean).join(" · ");
    const unban = el("button", { className: "btn small", title: `Unban ${b.name}` }, icon("check"), "Unban");
    unban.addEventListener("click", () => changeAccess("bans", "remove", b.name));
    bans.append(el("li", {}, avatar(b.name),
      el("div", { className: "grow" }, playerName(b.name), el("div", { className: "meta", textContent: meta })),
      unban));
  }
}

async function loadAccess() {
  const sid = ui.sid;
  try {
    const a = await api(S("/access"));
    if (sid !== ui.sid) return;
    ui.access = a;
    renderAccess();
    if (ui.status) renderStatus(ui.status); // refresh toggles in the online list
  } catch { /* shown elsewhere */ }
}

const isOnline = (n) => !!ui.status?.players.some((p) => lower(p) === lower(n));

/** The question asked before a whitelist / operator / ban change. */
function accessQuestion(kind, action, name) {
  const add = action === "add";
  switch (kind) {
    case "whitelist": return add
      ? { title: `Allow ${name} to join?`, text: `${name} is added to the whitelist.`, ok: "Allow", iconName: "user-plus" }
      : { title: `Remove ${name} from the whitelist?`, text: "They cannot join any more while the whitelist is on.", ok: "Remove", danger: true, iconName: "x" };
    case "ops": return add
      ? { title: `Make ${name} an operator?`, text: "Operators can use every command, including /stop and /op.", ok: "Make operator", danger: true, iconName: "crown-simple" }
      : { title: `Remove the operator rights of ${name}?`, ok: "Remove rights", iconName: "crown-simple" };
    default: return add
      ? { title: `Ban ${name}?`, text: isOnline(name) ? "They are kicked right away." : "", ok: "Ban", danger: true, iconName: "prohibit" }
      : { title: `Unban ${name}?`, text: "They can join again.", ok: "Unban", iconName: "check" };
  }
}

/** Change whitelist / operators / bans after asking (`asked`: the caller already asked, e.g. the
 *  ban reason prompt). */
async function changeAccess(kind, action, name, reason = "", asked = false) {
  if (!asked && !(await ask(accessQuestion(kind, action, name)))) return false;
  try {
    const res = await api(S(`/access/${kind}`), { method: "POST", body: { action, name, reason } });
    ui.access = res;
    toast(res.message);
    renderAccess();
    if (ui.status) renderStatus(ui.status);
    return true;
  } catch (err) {
    toast(err.message, true);
    return false;
  }
}

$$(".add-form").forEach((form) => form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const input = form.querySelector("input");
  const name = input.value.trim();
  if (!name) return;
  const kind = form.dataset.kind;
  const reason = kind === "bans" ? $("#ban-reason").value.trim() : "";
  if (await changeAccess(kind, "add", name, reason)) {
    input.value = "";
    if (kind === "bans") $("#ban-reason").value = "";
  }
}));

$("#wl-enabled").addEventListener("change", async (e) => {
  const on = e.target.checked;
  if (!(await ask(on ? { title: "Turn the whitelist on?", text: "Only players on the list can join then.", ok: "Turn on", iconName: "shield-check" }
                     : { title: "Turn the whitelist off?", text: "Everybody can join then.", ok: "Turn off", danger: true }))) {
    e.target.checked = !on;
    return;
  }
  try {
    ui.access = await api(S("/access/whitelist/enabled"), { method: "POST", body: { enabled: e.target.checked } });
    toast(`Whitelist ${ui.access.whitelist_enabled ? "enabled" : "disabled"}`);
    renderAccess();
  } catch (err) {
    e.target.checked = !e.target.checked;
    toast(err.message, true);
  }
});

function showPlayerTab(tab) {
  ui.ptab = tab;
  for (const t of $$("[data-ptab]")) t.setAttribute("aria-selected", String(t.dataset.ptab === tab));
  for (const p of $$(".players-card .tab-panel")) p.classList.toggle("hidden", p.dataset.tab !== tab);
}
$$("[data-ptab]").forEach((t) => t.addEventListener("click", () => {
  showPlayerTab(t.dataset.ptab);
  if (t.dataset.ptab !== "online") loadAccess();
}));

// ---------- server list icon ----------

function showServerIcon() {
  $("#server-icon-img").src = `${S("/icon")}?t=${Date.now()}`;
}

/** Any image -> 64×64 PNG (centre square, smooth scaling). */
async function iconPng(file) {
  const img = await createImageBitmap(file);
  const side = Math.min(img.width, img.height);
  const canvas = Object.assign(document.createElement("canvas"), { width: 64, height: 64 });
  const ctx = canvas.getContext("2d");
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(img, (img.width - side) / 2, (img.height - side) / 2, side, side, 0, 0, 64, 64);
  img.close();
  return new Promise((resolve, reject) => canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("Could not convert the image"))), "image/png"));
}

$("#server-icon-file").addEventListener("change", async (e) => {
  const file = e.target.files[0];
  e.target.value = "";
  if (!file || !(await ask({ title: "Change the server icon?", text: `Use ${file.name} as the icon in the server list?\nIt replaces the current icon.`, ok: "Use image", iconName: "upload-simple" }))) return;
  try {
    const res = await fetch(S("/icon"), { method: "PUT", body: await iconPng(file), credentials: "same-origin" });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(data.detail || `HTTP ${res.status}`);
    toast(ui.status?.state === "stopped" ? "Server icon saved" : "Server icon saved: shows after a restart");
    showServerIcon();
  } catch (err) {
    toast(err.message, true);
  }
});

$("#server-icon-reset").addEventListener("click", async () => {
  if (!(await ask({ title: "Reset the server icon?", text: "Use the panel logo as the icon in the server list again.", ok: "Reset" }))) return;
  try {
    await api(S("/icon"), { method: "DELETE" });
    toast("Server icon reset to the panel logo");
    showServerIcon();
  } catch (err) {
    toast(err.message, true);
  }
});

// ---------- player profile ----------

const profilePanel = $("#profile-panel");

/** Back to the hint in the profile card (nobody selected). */
function closeProfile() {
  ui.profile = null;
  $("#pf-body").classList.add("hidden");
  $("#pf-empty").classList.remove("hidden");
  if (ui.status) renderStatus(ui.status); // selection mark in the list
}
profilePanel.querySelector("[data-pf-close]").addEventListener("click", closeProfile);

function fmtAgo(ts) {
  if (!ts) return null;
  const sec = Math.max(0, Date.now() / 1000 - ts);
  if (sec < 90) return "just now";
  if (sec < 3600) return `${Math.round(sec / 60)} min ago`;
  if (sec < 86400 * 2) return `${Math.round(sec / 3600)} h ago`;
  return new Date(ts * 1000).toLocaleDateString();
}

function fmtPlayTime(sec) {
  const h = Math.floor(sec / 3600), m = Math.floor((sec % 3600) / 60);
  return h ? `${h} h ${m} min` : `${m} min`;
}

const DIM_NAME = { "minecraft:overworld": "Overworld", "minecraft:the_nether": "Nether", "minecraft:the_end": "End" };

async function openProfile(name) {
  ui.profile = { name };
  $("#pf-name").textContent = name;
  $("#pf-sub").textContent = "Loading…";
  $("#pf-head").replaceWith(Object.assign(avatar(name, false, true), { id: "pf-head" }));
  $("#pf-facts").replaceChildren();
  $("#pf-empty").classList.add("hidden");
  $("#pf-body").classList.remove("hidden");
  renderProfileButtons();
  if (ui.page !== "players") go(ui.sid, "players");
  if (ui.status) renderStatus(ui.status); // selection mark in the list
  if (innerWidth <= 1100) profilePanel.scrollIntoView({ behavior: "smooth", block: "start" });
  const sid = ui.sid;
  try {
    const p = await api(S(`/players/${encodeURIComponent(name)}`));
    if (sid !== ui.sid || ui.profile?.name !== name) return;
    ui.profile = p;
    renderProfile();
  } catch (err) {
    $("#pf-sub").textContent = err.message;
  }
}

function renderProfile() {
  const p = ui.profile;
  $("#pf-name").textContent = p.name;
  $("#pf-sub").textContent = [p.online ? "online now" : p.last_seen ? `last seen ${fmtAgo(p.last_seen)}` : "not seen by the panel yet",
                              p.ban ? "banned" : null].filter(Boolean).join(" · ");
  const facts = [];
  const add = (label, value) => { if (value != null && value !== "") facts.push(el("div", {}, el("dt", { textContent: label }), el("dd", { textContent: value }))); };
  add("Play time", p.stats ? fmtPlayTime(p.stats.play_time) : null);
  add("First seen", p.first_seen ? new Date(p.first_seen * 1000).toLocaleString() : null);
  add("Game mode", p.gamemode ? p.gamemode[0].toUpperCase() + p.gamemode.slice(1) : null);
  add("Last position", p.pos ? `${p.pos.map(Math.round).join(" / ")} · ${DIM_NAME[p.dimension] || p.dimension || ""}` : null);
  add("Health · level", p.health != null ? `${p.health / 2} ♥ · level ${p.level ?? 0}` : null);
  add("Deaths · mob kills", p.stats ? `${p.stats.deaths} · ${p.stats.mob_kills}` : null);
  add("Walked, flown, swum", p.stats ? `${p.stats.distance_km} km` : null);
  add("Ban", p.ban ? [p.ban.reason, p.ban.source ? `by ${p.ban.source}` : null].filter(Boolean).join(" · ") : null);
  add("UUID", p.uuid);
  $("#pf-facts").replaceChildren(...facts);
  for (const r of $$('input[name="pf-gm"]')) r.checked = r.value === p.gamemode;
  renderProfileButtons();
}

function renderProfileButtons() {
  const p = ui.profile;
  if (!p) return;
  const online = ui.status?.players.some((n) => lower(n) === lower(p.name));
  $("#pf-live").classList.toggle("hidden", !online);
  $("#pf-offline").classList.toggle("hidden", !!online);
  const btn = (k) => profilePanel.querySelector(`[data-pf="${k}"]`);
  const wl = onWhitelist(p.name), op = isOp(p.name), ban = banOf(p.name) || p.ban;
  btn("whitelist").replaceChildren(icon("list-checks"), wl ? "Remove from whitelist" : "Add to whitelist");
  btn("op").replaceChildren(icon("crown-simple"), op ? "Remove operator" : "Make operator");
  btn("ban").replaceChildren(icon("prohibit"), ban ? "Unban" : "Ban…");
  btn("ban").classList.toggle("danger", !ban);
}

/** The question asked before an action on an online player (kick asks for a reason instead). */
const cap = (s) => s ? s[0].toUpperCase() + s.slice(1) : s;
const PLAYER_QUESTIONS = {
  gamemode: (n, v) => ({
    title: `${cap(v)} mode for ${n}?`,
    text: ui.profile?.gamemode ? `${cap(ui.profile.gamemode)} → ${cap(v)}` : "",
    ok: `Switch to ${cap(v)}`,
  }),
  spawn: (n) => ({ title: `Teleport ${n} to the world spawn?`, ok: "Teleport", iconName: "house" }),
  heal: (n) => ({ title: `Heal ${n}?`, text: "Full health and food.", ok: "Heal", iconName: "heart" }),
  clear: (n) => ({ title: `Clear the inventory of ${n}?`, text: "All items are gone for good.", ok: "Clear inventory", danger: true, iconName: "trash" }),
};

async function playerAction(action, value = "", asked = false) {
  const name = ui.profile?.name;
  const question = PLAYER_QUESTIONS[action];
  if (!asked && question && !(await ask(question(name, value)))) return false;
  try {
    const res = await api(S(`/players/${encodeURIComponent(name)}/action`), { method: "POST", body: { action, value } });
    toast(res.message);
    return true;
  } catch (err) {
    toast(err.message, true);
    return false;
  }
}

$$('input[name="pf-gm"]').forEach((r) => r.addEventListener("change", async () => {
  if (await playerAction("gamemode", r.value)) {
    if (ui.profile) ui.profile.gamemode = r.value;
  } else {
    for (const x of $$('input[name="pf-gm"]')) x.checked = x.value === ui.profile?.gamemode; // cancelled: back
  }
}));

profilePanel.querySelectorAll("[data-pf]").forEach((b) => b.addEventListener("click", async () => {
  const name = ui.profile?.name;
  if (!name) return;
  switch (b.dataset.pf) {
    case "spawn": return playerAction("spawn");
    case "heal": return playerAction("heal");
    case "clear": return playerAction("clear");
    case "kick": {
      const reason = await ask({ title: `Kick ${name}?`, text: "They can join again right away.", ok: "Kick", danger: true,
        iconName: "sign-out", input: { label: "Reason (optional)", placeholder: "Shown to the player" } });
      if (reason !== null) playerAction("kick", reason, true);
      return;
    }
    case "whitelist": await changeAccess("whitelist", onWhitelist(name) ? "remove" : "add", name); break;
    case "op": await changeAccess("ops", isOp(name) ? "remove" : "add", name); break;
    case "ban": {
      if (banOf(name) || ui.profile.ban) {
        await changeAccess("bans", "remove", name);
      } else {
        const reason = await ask({ ...accessQuestion("bans", "add", name), input: { label: "Reason (optional)", placeholder: "Shown to the player" } });
        if (reason === null) return;
        await changeAccess("bans", "add", name, reason, true);
      }
      break;
    }
  }
  if (ui.profile) openProfile(name);
}));

// ---------- data packs ----------

const PACK_STATUS = {
  enabled: "enabled", disabled: "disabled", new: "enabled on next start", null: "not in this world",
};

function packButton(iconName, title, onClick, { danger = false, disabled = false } = {}) {
  const b = el("button", { className: `icon-btn small${danger ? " danger" : ""}`, title, ariaLabel: title, disabled }, icon(iconName));
  b.addEventListener("click", onClick);
  return b;
}

function renderDatapacks(d) {
  ui.datapacks = d;
  const ul = $("#datapacks");
  ul.replaceChildren();
  $("#addon-note").textContent = d.experiments?.enabled.length
    ? `Experiments in this world: ${d.experiments.enabled.join(", ")} (chosen when the world was created).`
    : "";
  if (!d.packs.length) {
    ul.append(el("li", { className: "empty", textContent: "No data packs yet — upload a .zip" }));
    return;
  }
  const alive = ui.status && ui.status.state !== "stopped";
  // Grouped by the optional pack.mcmeta "panel.category" (packs without one come last)
  const other = "Other";
  const groups = new Map();
  for (const p of d.packs) {
    const c = p.category || other;
    if (!groups.has(c)) groups.set(c, []);
    groups.get(c).push(p);
  }
  const order = [...groups.keys()].sort((a, b) => (a === other) - (b === other) || a.localeCompare(b));
  const showHeads = order.length > 1 || order[0] !== other;
  for (const c of order) {
    if (showHeads) ul.append(el("li", { className: "group-head", textContent: c }));
    for (const p of groups.get(c)) renderDatapack(ul, p, alive);
  }
}

function renderDatapack(ul, p, alive) {
  {
    const label = p.title || p.name.replace(/\.zip$/i, "");
    // Every pack gets a switch: switching on a library pack that is not in the world installs it first
    const on = p.installed && p.status !== "disabled";
    const toggle = p.installed || p.in_library
      ? switchInput(on, on ? `Disable ${label} in this world` : `Enable ${label} in this world`,
          (input) => toggleDatapack(p, input), !p.valid)
      : null;
    // On/off is what the switch shows; only say what it cannot (waiting for the next start)
    const meta = [p.status === "new" ? PACK_STATUS.new : null, p.in_library ? null : "only in this world", `${p.size_kb} KB`].filter(Boolean).join(" · ");
    let iconBox;
    if (p.icon) {
      // pack.png; the size in the URL refreshes it after an update
      iconBox = el("img", { className: "pack-icon pixel", alt: "", loading: "lazy",
        src: S(`/datapacks/${encodeURIComponent(p.name)}/icon?v=${p.size_kb}`) });
    } else {
      iconBox = el("span", { className: "pack-icon", textContent: label[0]?.toUpperCase() || "?" });
      iconBox.style.background = nameColor(label);
    }
    iconBox.setAttribute("aria-hidden", "true");
    ul.append(el("li", { className: p.installed ? "" : "inactive", dataset: { pack: p.name } },
      iconBox,
      el("div", { className: "grow" },
        el("div", { className: "title", textContent: label },
          p.title ? el("span", { className: "pack-id", textContent: p.name }) : null),
        p.description ? el("div", { className: "meta", textContent: p.description }) : null,
        el("div", { className: "meta", textContent: meta })),
      p.valid ? null : tag("no pack.mcmeta", "warn", "warning"),
      p.outdated ? tag("update", "info") : null,
      el("span", { className: "row-actions" },
        p.in_library && p.outdated
          ? packButton("download-simple", `Update ${label} in this world from the library`,
              () => packAction(p, "install", "POST", "updated"))
          : null,
        p.in_library ? null
          : packButton("archive", `Copy ${label} to the library`, () => packAction(p, "adopt", "POST", "copied to the library")),
        p.installed
          ? packButton("x", alive ? "Stop the server to remove it from the world" : `Remove ${label} from this world`,
              () => removeDatapack(p), { danger: true, disabled: alive })
          : null,
        p.in_library ? packButton("trash", `Delete ${label} from the library`, () => deleteDatapack(p), { danger: true }) : null),
      toggle));
  }
}

async function loadDatapacks() {
  const sid = ui.sid;
  try {
    const d = await api(S("/datapacks"));
    if (sid === ui.sid) renderDatapacks(d);
  } catch (err) {
    toast(err.message, true);
  }
}

const PACK_BUSY = { install: "Updating…", adopt: "Copying to the library…", world: "Removing…", library: "Deleting…" };

// While an action runs (it can take seconds: /reload and saving until level.dat shows the result),
// the pack's row shows what is happening and its controls are locked; the next render ends it.
function packBusy(p, text) {
  const li = $("#datapacks").querySelector(`li[data-pack="${CSS.escape(p.name)}"]`);
  if (!li) return;
  li.classList.add("busy");
  li.querySelectorAll("button, input").forEach((c) => { c.disabled = true; });
  li.querySelector(".row-actions")?.prepend(
    el("span", { className: "busy-note", role: "status" }, el("span", { className: "spinner" }), text));
}

async function packAction(p, action, method, done) {
  packBusy(p, PACK_BUSY[action] || "Working…");
  try {
    renderDatapacks(await api(S(`/datapacks/${encodeURIComponent(p.name)}/${action}`), { method }));
    toast(`${p.title || p.name} ${done}`);
  } catch (err) {
    toast(err.message, true);
    if (ui.datapacks) renderDatapacks(ui.datapacks);
  }
}

async function toggleDatapack(p, input) {
  const enabled = input.checked;
  input.disabled = true;
  packBusy(p, enabled ? (p.installed ? "Enabling…" : "Installing…") : "Disabling…");
  const url = (action) => S(`/datapacks/${encodeURIComponent(p.name)}/${action}`);
  try {
    if (enabled && !p.installed) {
      // Copy it into the world (enabled on /reload or the next start); a pack that was disabled in
      // this world before stays disabled after installing, so enable it explicitly then
      const d = await api(url("install"), { method: "POST" });
      const now = d.packs.find((x) => x.name === p.name);
      renderDatapacks(now?.status === "disabled"
        ? await api(url("enabled"), { method: "POST", body: { enabled } })
        : d);
      toast(`${p.title || p.name} enabled`);
      return;
    }
    renderDatapacks(await api(url("enabled"), { method: "POST", body: { enabled } }));
    toast(`${p.title || p.name} ${enabled ? "enabled" : "disabled"}`);
  } catch (err) {
    toast(err.message, true);
    if (ui.datapacks) renderDatapacks(ui.datapacks);
  }
}

function removeDatapack(p) {
  const note = p.in_library ? "It stays in the library." : "It is NOT in the library, so it will be gone (copy it to the library first to keep it).";
  ask({ title: `Remove ${p.name} from this world?`, text: note, ok: "Remove", danger: true, iconName: "x" })
    .then((yes) => yes && packAction(p, "world", "DELETE", "removed from the world"));
}

function deleteDatapack(p) {
  const note = p.installed ? "The copy in the current world stays." : "";
  ask({ title: `Delete ${p.name} from the library?`, text: `It is removed for all servers.\n${note}`, ok: "Delete", danger: true, iconName: "trash" })
    .then((yes) => yes && packAction(p, "library", "DELETE", "deleted from the library"));
}

async function uploadDatapack(file) {
  if (!file) return;
  if (!/\.zip$/i.test(file.name)) return toast("Upload a .zip file", true);
  toast(`Uploading ${file.name}…`);
  try {
    const res = await fetch(S(`/datapacks/upload?name=${encodeURIComponent(file.name)}`), {
      method: "POST", body: file, headers: { "Content-Type": "application/zip" }, credentials: "same-origin",
    });
    if (res.status === 401) return showLogin();
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(data.detail || `HTTP ${res.status}`);
    renderDatapacks(data);
    toast(`${file.name} added`);
  } catch (err) {
    toast(err.message, true);
  }
}

$("#datapack-file").addEventListener("change", (e) => {
  const file = e.target.files[0];
  e.target.value = "";
  uploadDatapack(file);
});
/** Drag & drop onto a .dropzone label: hand the file to the upload function of its input. */
function dropTarget(zone, upload) {
  zone.addEventListener("dragover", (e) => { e.preventDefault(); zone.classList.add("over"); });
  zone.addEventListener("dragleave", () => zone.classList.remove("over"));
  zone.addEventListener("drop", (e) => {
    e.preventDefault();
    zone.classList.remove("over");
    upload(e.dataTransfer.files[0]);
  });
}
dropTarget($('.dropzone[for="datapack-file"]'), uploadDatapack);

// ---------- mods (Fabric) & plugins (Paper) ----------

const KIND_LABEL = { mods: ["Mods", "mod"], plugins: ["Plugins", "plugin"] };

function showAddonTab(tab) {
  ui.atab = tab;
  store("panel.addonTab", tab);
  for (const t of $$("[data-atab]")) t.setAttribute("aria-selected", String(t.dataset.atab === tab));
  $("#datapacks-panel").classList.toggle("hidden", tab !== "datapacks");
  $("#jars-panel").classList.toggle("hidden", tab !== "jars");
  if (tab !== "jars") return;
  // Show what we already have at once; before the first answer show nothing rather than a
  // placeholder layout that jumps (empty list ↔ Vanilla note) when the data arrives
  if (ui.addons) renderAddons(ui.addons);
  else for (const id of ["#vanilla-note", "#jars-content"]) $(id).classList.add("hidden");
  loadAddons();
}
$$("[data-atab]").forEach((t) => t.addEventListener("click", () => showAddonTab(t.dataset.atab)));
$$("[data-goto]").forEach((a) => a.addEventListener("click", (e) => { e.preventDefault(); go(ui.sid, a.dataset.goto); }));

/** Tab label follows the server type (Mods for Fabric, Plugins for Paper). */
function renderAddonTabLabel() {
  const kind = { fabric: "mods", paper: "plugins" }[ui.settings?.flavor];
  $("#jars-tab-label").textContent = kind ? KIND_LABEL[kind][0] : "Mods & plugins";
}

function renderAddons(d) {
  ui.addons = d;
  const vanilla = !d.kind;
  $("#vanilla-note").classList.toggle("hidden", !vanilla);
  $("#jars-content").classList.toggle("hidden", vanilla);
  $("#count-jars").textContent = d.items.length || "";
  if (vanilla) return;
  const [Title, one] = KIND_LABEL[d.kind];
  $("#jars-title").textContent = `${Title} on this server`;
  $("#jars-note").textContent = d.running ? "changes apply after a restart" : "";
  // Modrinth only for Fabric mods: for plugins the panel never contacts it (upload only)
  const modrinth = d.kind === "mods";
  $("#mr-box").classList.toggle("hidden", !modrinth);
  $("#upload-head").classList.toggle("hidden", modrinth);
  $("#jars-updates").classList.toggle("hidden", !modrinth);
  $("#jar-drop-note").textContent = modrinth ? `${one} .jar from another source` : `${one} .jar`;
  $("#mr-note").textContent = d.version ? `${d.kind === "mods" ? "Fabric" : "Paper"} ${d.version}, only server-compatible projects`
    : "Choose a Minecraft version first";
  const ul = $("#jars");
  ul.replaceChildren();
  if (!d.items.length) {
    ul.append(el("li", { className: "empty", textContent: `No ${Title.toLowerCase()} yet — ${modrinth ? "search Modrinth or " : ""}upload a .jar` }));
    return;
  }
  const updates = ui.addonUpdates || {};
  for (const a of d.items) {
    const upd = updates[a.file];
    const meta = [a.version, a.authors.length ? `by ${a.authors.join(", ")}` : null, `${a.size_kb} KB`].filter(Boolean).join(" · ");
    const iconBox = el("span", { className: "pack-icon", textContent: a.name[0]?.toUpperCase() || "?" });
    iconBox.style.background = nameColor(a.name);
    iconBox.setAttribute("aria-hidden", "true");
    const tags = [];
    if (a.side === "both") tags.push(tag("players need it too", "violet", "users"));
    else if (a.side === "server") tags.push(tag("server only", "accent"));
    else if (a.side === "client") tags.push(tag("client only", "warn", "warning"));
    for (const p of a.problems) tags.push(tag(p, "danger", "warning"));
    if (a.missing.length) tags.push(tag(`needs ${a.missing.join(", ")}`, "danger", "warning"));
    if (a.incompatible) tags.push(tag(a.incompatible, "warn", "warning"));
    if (upd) tags.push(tag(`update: ${upd.version}`, "info"));
    const del = packButton("trash", `Delete ${a.name}`, () => deleteAddon(a), { danger: true });
    const updBtn = upd ? packButton("download-simple", `Update ${a.name} to ${upd.version}`, () => updateAddon(a)) : null;
    ul.append(el("li", { className: a.enabled ? "" : "inactive" },
      iconBox,
      el("div", { className: "grow" },
        el("div", { className: "title", textContent: a.name }),
        a.description ? el("div", { className: "meta", textContent: a.description }) : null,
        el("div", { className: "meta", textContent: meta }),
        tags.length ? el("div", { className: "tag-row" }, ...tags) : null),
      el("span", { className: "row-actions" }, updBtn, del),
      switchInput(a.enabled, a.enabled ? `Disable ${a.name}` : `Enable ${a.name}`, (input) => toggleAddon(a, input))));
  }
}

async function loadAddons() {
  const sid = ui.sid;
  try {
    const d = await api(S("/addons"));
    if (sid !== ui.sid) return;
    renderAddons(d);
    // First visit: show popular projects for this version
    if (d.kind === "mods" && d.version && !$("#mr-results").childElementCount) searchModrinth();
  } catch (err) {
    toast(err.message, true);
  }
}

async function addonAction(path, method, done, body) {
  try {
    renderAddons(await api(S(path), { method, body }));
    toast(done);
  } catch (err) {
    toast(err.message, true);
    loadAddons();
  }
}

const toggleAddon = (a, input) => {
  input.disabled = true;
  addonAction(`/addons/${encodeURIComponent(a.file)}/enabled`, "POST", `${a.name} ${input.checked ? "enabled" : "disabled"}`, { enabled: input.checked });
};

function deleteAddon(a) {
  ask({ title: `Delete ${a.name}?`, text: `${a.file} is removed from this server.`, ok: "Delete", danger: true, iconName: "trash" })
    .then((yes) => yes && addonAction(`/addons/${encodeURIComponent(a.file)}`, "DELETE", `${a.name} deleted`));
}

async function updateAddon(a) {
  toast(`Updating ${a.name}…`);
  await addonAction(`/addons/${encodeURIComponent(a.file)}/update`, "POST", `${a.name} updated`);
  if (ui.addonUpdates) delete ui.addonUpdates[a.file];
  if (ui.addons) renderAddons(ui.addons);
}

$("#jars-updates").addEventListener("click", async () => {
  const btn = $("#jars-updates");
  btn.disabled = true;
  try {
    ui.addonUpdates = await api(S("/addons/updates"));
    const n = Object.keys(ui.addonUpdates).length;
    toast(n ? `${n} update${n > 1 ? "s" : ""} available` : "Everything from Modrinth is up to date");
    if (ui.addons) renderAddons(ui.addons);
  } catch (err) {
    toast(err.message, true);
  } finally {
    btn.disabled = false;
  }
});

function fmtCount(n) {
  return n >= 1e6 ? `${(n / 1e6).toFixed(1)} M` : n >= 1e3 ? `${Math.round(n / 1e3)} k` : String(n);
}

async function searchModrinth() {
  const sid = ui.sid;
  const ul = $("#mr-results");
  ul.replaceChildren(el("li", { className: "empty", textContent: "Searching…" }));
  try {
    const res = await api(S(`/addons/search?q=${encodeURIComponent($("#mr-query").value)}`));
    if (sid !== ui.sid) return;
    ul.replaceChildren();
    if (!res.hits.length) ul.append(el("li", { className: "empty", textContent: "Nothing found for this version" }));
    for (const h of res.hits) {
      const img = h.icon_url
        ? el("img", { className: "pack-icon", src: h.icon_url, alt: "", loading: "lazy", referrerPolicy: "no-referrer" })
        : el("span", { className: "pack-icon", textContent: h.title[0]?.toUpperCase() || "?" });
      const add = el("button", { className: "btn small", title: `Install ${h.title} and its required dependencies` }, icon("download-simple"), "Install");
      add.addEventListener("click", async () => {
        add.disabled = true;
        toast(`Installing ${h.title}…`);
        try {
          const d = await api(S("/addons/install"), { method: "POST", body: { project_id: h.project_id } });
          renderAddons(d);
          toast(d.installed.length ? `Installed: ${d.installed.join(", ")}` : `${h.title} is already installed`);
        } catch (err) {
          toast(err.message, true);
        } finally {
          add.disabled = false;
        }
      });
      const side = h.client_side === "required" && ui.addons?.kind === "mods" ? " · players need it too" : "";
      ul.append(el("li", {}, img,
        el("div", { className: "grow" },
          el("a", { className: "title", href: `https://modrinth.com/project/${encodeURIComponent(h.slug)}`, target: "_blank", rel: "noopener", textContent: h.title }),
          el("div", { className: "meta clamp", textContent: h.description }),
          el("div", { className: "meta", textContent: `${fmtCount(h.downloads)} downloads${side}` })),
        add));
    }
  } catch (err) {
    ul.replaceChildren();
    toast(err.message, true);
  }
}

$("#mr-form").addEventListener("submit", (e) => { e.preventDefault(); searchModrinth(); });

async function uploadJar(file) {
  if (!file) return;
  if (!/\.jar$/i.test(file.name)) return toast("Upload a .jar file", true);
  toast(`Uploading ${file.name}…`);
  try {
    const res = await fetch(S(`/addons/upload?name=${encodeURIComponent(file.name)}`), {
      method: "POST", body: file, headers: { "Content-Type": "application/java-archive" }, credentials: "same-origin",
    });
    if (res.status === 401) return showLogin();
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(data.detail || `HTTP ${res.status}`);
    renderAddons(data);
    toast(`${file.name} added`);
  } catch (err) {
    toast(err.message, true);
  }
}

$("#jar-file").addEventListener("change", (e) => {
  const file = e.target.files[0];
  e.target.value = "";
  uploadJar(file);
});
dropTarget($('.dropzone[for="jar-file"]'), uploadJar);

// ---------- resource history (last hour) ----------

async function loadHistory() {
  const sid = ui.sid;
  try {
    const h = await api(S("/history"));
    if (sid !== ui.sid) return;
    const pts = h.points;
    const n = Math.round(3600 / h.interval);
    $("#history-card").classList.toggle("hidden", pts.length < 2);
    if (pts.length < 2) return;
    const col = (k) => pts.map((p) => p[k]);
    const cpu = col("cpu"), ram = col("ram"), players = col("players");
    const tps = pts.map((p) => p.tps ?? 20);
    drawSpark("#hist-cpu", cpu, Math.max(10, ...cpu) * 1.15, n);
    drawSpark("#hist-ram", ram, Math.max(...ram) * 1.1, n);
    drawSpark("#hist-players", players, Math.max(1, ...players) * 1.15, n);
    drawSpark("#hist-tps", tps, 20, n);
    $("#h-cpu").textContent = `max ${Math.max(...cpu).toFixed(1)} %`;
    $("#h-ram").textContent = `max ${fmtMB(Math.max(...ram))}`;
    $("#h-players").textContent = `max ${Math.max(...players)}`;
    const measured = pts.filter((p) => p.tps != null).map((p) => p.tps);
    $("#h-tps").textContent = measured.length ? `min ${Math.min(...measured).toFixed(1)}` : "not measured";
  } catch { /* not important */ }
}

setInterval(() => {
  if (ui.page === "overview" && ui.status?.state === "running" && !document.hidden && !$("#app").classList.contains("hidden")) loadHistory();
}, 30000);

// ---------- game rules ----------

const rulesDialog = $("#rules-dialog");
rulesDialog.querySelectorAll("[data-close]").forEach((b) => b.addEventListener("click", () => rulesDialog.close()));

function ruleRow(r) {
  let control;
  if (r.type === "bool") {
    control = switchInput(r.value, r.command, (input) => setRule(r, input.checked, input));
  } else {
    const input = el("input", { type: "number", value: r.value, step: 1, className: "rule-num", ariaLabel: r.command });
    input.addEventListener("change", () => setRule(r, Number(input.value), input));
    control = input;
  }
  return el("div", { className: "rule", dataset: { search: `${r.command} ${r.label || ""}`.toLowerCase() } },
    el("div", { className: "grow" },
      el("div", { className: "title", textContent: r.label || r.command }),
      r.label ? el("div", { className: "meta mono", textContent: r.command }) : null),
    control);
}

function renderRules(d) {
  const list = $("#rules-list");
  const known = d.rules.filter((r) => r.label), rest = d.rules.filter((r) => !r.label);
  list.replaceChildren(
    known.length ? el("div", { className: "label", textContent: "Common" }) : null, ...known.map(ruleRow),
    rest.length ? el("div", { className: "label", textContent: "All" }) : null, ...rest.map(ruleRow));
  if (!d.world) list.replaceChildren(el("p", { className: "muted", textContent: "The world doesn't exist yet: start the server once." }));
  $("#rules-note").textContent = d.live ? "Live from the running server: changes apply immediately."
    : ui.status?.state === "stopped" ? "Server stopped: changes are written to the world and apply on the next start."
    : "Values as of the last world save.";
  filterRules();
}

function filterRules() {
  const q = $("#rules-search").value.trim().toLowerCase();
  for (const row of $$("#rules-list .rule")) row.classList.toggle("hidden", !!q && !row.dataset.search.includes(q));
  for (const lbl of $$("#rules-list > .label")) lbl.classList.toggle("hidden", !!q);
}
$("#rules-search").addEventListener("input", filterRules);

async function setRule(r, value, input) {
  if (r.type === "int" && !Number.isInteger(value)) { toast("Enter a whole number", true); input.value = r.value; return; }
  input.disabled = true;
  try {
    await api(S("/gamerules"), { method: "POST", body: { name: r.name, value } });
    r.value = value;
    toast(`${r.label || r.command}: ${typeof value === "boolean" ? (value ? "on" : "off") : value}`);
  } catch (err) {
    if (r.type === "bool") input.checked = r.value; else input.value = r.value;
    toast(err.message, true);
  } finally {
    input.disabled = false;
  }
}

$("#rules-btn").addEventListener("click", async () => {
  $("#rules-search").value = "";
  $("#rules-list").replaceChildren(el("p", { className: "muted", textContent: "Loading…" }));
  $("#rules-note").textContent = "";
  rulesDialog.showModal();
  try {
    renderRules(await api(S("/gamerules")));
  } catch (err) {
    $("#rules-list").replaceChildren(el("p", { className: "error", textContent: err.message }));
  }
});

// ---------- world upload ----------

function uploadWorld(file) {
  if (!file) return;
  if (!/\.zip$/i.test(file.name)) return toast("Upload the world as a .zip file", true);
  const xhr = new XMLHttpRequest();
  xhr.open("POST", S("/backups/upload"));
  xhr.setRequestHeader("Content-Type", "application/zip");
  let last = 0;
  xhr.upload.onprogress = (e) => {
    if (!e.lengthComputable || Date.now() - last < 700) return;
    last = Date.now();
    toast(`Uploading ${file.name}… ${Math.round((e.loaded / e.total) * 100)} %`);
  };
  xhr.onload = () => {
    if (xhr.status === 401) return showLogin();
    let data = {};
    try { data = JSON.parse(xhr.responseText); } catch { /* not JSON */ }
    if (xhr.status >= 300) return toast(data.detail || `HTTP ${xhr.status}`, true);
    renderBackups(data.backups);
    toast("World uploaded: it is in the list as a backup — press Restore to use it");
  };
  xhr.onerror = () => toast("Upload failed", true);
  toast(`Uploading ${file.name}…`);
  xhr.send(file);
}

$("#world-file").addEventListener("change", (e) => {
  const file = e.target.files[0];
  e.target.value = "";
  uploadWorld(file);
});

// ---------- backups & world ----------

function renderBackups(list, keep, interval) {
  if (keep != null) {
    $("#backup-keep").textContent = (interval ? `every ${interval} h · ` : "") + `keeps ${keep}`;
  }
  const ul = $("#backups");
  ul.replaceChildren();
  if (!list.length) ul.append(el("li", { className: "empty", textContent: "No backups yet" }));
  for (const b of list) {
    const link = el("a", {
      className: "icon-btn small", href: S(`/backups/${encodeURIComponent(b.name)}`),
      title: "Download", ariaLabel: `Download backup ${b.created}`,
    }, icon("download-simple"));
    const restore = el("button", { className: "btn small", title: "Restore this backup" }, icon("clock-counter-clockwise"), "Restore");
    restore.addEventListener("click", () => restoreBackup(b));
    const kind = b.archive ? tag("Archive", "violet", "archive")
      : b.note === "before restore" ? tag("Safety", "warn") : tag("Backup");
    ul.append(el("li", {},
      kind,
      el("div", { className: "grow" },
        el("div", { className: "title", textContent: b.created.replace("T", "  ") }),
        el("div", { className: "meta", textContent: `${b.size_mb} MB` + (b.note ? ` · ${b.note}` : "") })),
      restore, link));
  }
}

async function restoreBackup(b) {
  if (ui.status?.state !== "stopped") return toast("Stop the server before restoring a backup", true);
  if (!(await ask({ title: "Restore this backup?", text: `Backup from ${b.created.replace("T", " ")}.\n` +
                    "The current world is replaced. It is saved as a new backup first, so you can go back.",
                    ok: "Restore", danger: true, iconName: "clock-counter-clockwise" }))) return;
  try {
    await api(S(`/backups/${encodeURIComponent(b.name)}/restore`), { method: "POST" });
    toast("Restoring…");
  } catch (err) {
    toast(err.message, true);
  }
}

$("#map-render").addEventListener("click", async () => {
  try {
    await api(S("/map/render"), { method: "POST" });
    toast(ui.status?.state === "running" ? "Saving the world, then updating the map…" : "Updating the map…");
  } catch (err) {
    toast(err.message, true);
  }
});

async function loadCaveReach() {
  const form = $("#cave-form");
  form.classList.toggle("hidden", !ui.server?.has_map);
  if (!ui.server?.has_map) return;
  try {
    const v = await api("/api/map/visibility");
    $("#cave-reach").value = v.cave_reach;
    $("#all-caves").checked = !!v.all_caves;
  } catch { form.classList.add("hidden"); }
}

async function saveVisibility(body) {
  try {
    const v = await api("/api/map/visibility", { method: "PUT", body });
    $("#cave-reach").value = v.cave_reach;
    $("#all-caves").checked = !!v.all_caves;
    toast("Saved — the map is re-rendered");
  } catch (err) {
    toast(err.message, true);
    loadCaveReach();
  }
}

$("#cave-form").addEventListener("submit", (e) => {
  e.preventDefault();
  saveVisibility({ cave_reach: Number($("#cave-reach").value) });
});
$("#all-caves").addEventListener("change", async (e) => {
  if (e.target.checked && !(await ask({ title: "Show all caves with the height cut?", text: "Everybody who opens the map can then see hidden caves (and bases in them).", ok: "Show caves", danger: true }))) {
    e.target.checked = false;
    return;
  }
  saveVisibility({ all_caves: e.target.checked });
});

$("#map-enabled").addEventListener("change", async (e) => {
  try {
    await api(S("/settings"), { method: "POST", body: { map_enabled: e.target.checked } });
    ui.settings.map_enabled = e.target.checked;
    if (ui.status) renderStatus(ui.status);
    toast(`Live map ${e.target.checked ? "on" : "off"}`);
  } catch (err) {
    e.target.checked = !e.target.checked;
    toast(err.message, true);
  }
});

// ---------- waypoints of the live map (waypoints.py) ----------

const WP_COLOR = { green: "#7fc8a4", blue: "#6cb8d4", red: "#e0605d", yellow: "#f1d36b", purple: "#c58cf0", orange: "#f4a259", white: "#e8ecf2" };
const WP_DIMS = { overworld: "Overworld", the_nether: "Nether", the_end: "End" };
let wpData = null, wpEditing = null, wpColor = "green";

async function loadWaypoints() {
  const card = $("#wp-card");
  card.classList.toggle("hidden", !ui.server?.has_map);
  if (!ui.server?.has_map) return;
  const sid = ui.sid;
  try {
    const d = await api(S("/waypoints"));
    if (sid !== ui.sid) return;
    wpData = d;
    renderWaypoints();
  } catch (err) {
    toast(err.message, true);
  }
}

function renderWaypoints() {
  const list = [...wpData.waypoints].sort((a, b) => a.dim.localeCompare(b.dim) || a.name.localeCompare(b.name));
  $("#wp-count").textContent = list.length ? `· ${list.length}` : "";
  const ul = $("#wp-list");
  ul.replaceChildren();
  if (!list.length) ul.append(el("li", { className: "empty", textContent: "No waypoints yet" }));
  for (const w of list) {
    const where = `${w.x} ${w.y ?? "~"} ${w.z} · ${WP_DIMS[w.dim] || w.dim}`;
    const show = el("a", {
      className: "icon-btn small", target: "_blank", rel: "noopener", title: "Show on the live map",
      ariaLabel: `Show ${w.name} on the live map`,
    }, icon("map-trifold"));
    if (ui.mapUrl) show.href = `${ui.mapUrl}#${w.dim}/${w.x}/${w.z}/2`;
    else show.classList.add("hidden");
    const editBtn = el("button", { type: "button", className: "icon-btn small", title: "Edit", ariaLabel: `Edit ${w.name}` }, icon("pencil"));
    editBtn.addEventListener("click", () => openWaypointDialog(w));
    const del = el("button", { type: "button", className: "icon-btn small", title: "Delete", ariaLabel: `Delete ${w.name}` }, icon("trash"));
    del.addEventListener("click", () => deleteWaypoint(w));
    const dot = el("span", { className: "wp-dot" });
    dot.style.background = WP_COLOR[w.color] || WP_COLOR.green;
    ul.append(el("li", {},
      dot,
      el("div", { className: "grow" },
        el("div", { className: "title", textContent: w.name }),
        el("div", { className: "meta", textContent: w.owner ? `${where} · ${w.owner}` : where })),
      el("div", { className: "row-actions" }, show, editBtn, del)));
  }
}

async function deleteWaypoint(w) {
  if (!await ask({ title: "Delete waypoint?", text: `"${w.name}" disappears from the live map for everybody.`, ok: "Delete", danger: true, iconName: "trash" })) return;
  try {
    await api(S(`/waypoints/${encodeURIComponent(w.id)}`), { method: "DELETE" });
    toast(`Deleted "${w.name}"`);
  } catch (err) {
    toast(err.message, true);
  }
  loadWaypoints();
}

const wpDialog = $("#wp-dialog");

function openWaypointDialog(w = null) {
  wpEditing = w;
  $("#wp-title").textContent = w ? "Edit waypoint" : "New waypoint";
  $("#wp-name").value = w ? w.name : "";
  const colors = wpData?.colors || Object.keys(WP_COLOR);
  wpColor = w ? w.color : colors[0];
  $("#wp-colors").replaceChildren(...colors.map((c) => {
    const b = el("button", { type: "button", className: `wp-swatch${c === wpColor ? " on" : ""}`, title: c, ariaLabel: c });
    b.style.background = WP_COLOR[c] || "#888";
    b.addEventListener("click", () => {
      wpColor = c;
      for (const s of $("#wp-colors").children) s.classList.toggle("on", s === b);
    });
    return b;
  }));
  const dims = { ...WP_DIMS };
  if (w && !dims[w.dim]) dims[w.dim] = w.dim;
  $("#wp-dim").replaceChildren(...Object.entries(dims).map(([v, label]) => new Option(label, v, false, v === (w?.dim || "overworld"))));
  $("#wp-x").value = w ? w.x : 0;
  $("#wp-y").value = w?.y ?? "";
  $("#wp-z").value = w ? w.z : 0;
  // the owner is chosen when adding; a waypoint keeps it
  $("#wp-owner-wrap").classList.toggle("hidden", !!w);
  $("#wp-owner").replaceChildren(new Option("Nobody (set in the panel)", ""), ...(wpData?.players || []).map((n) => new Option(n, n)));
  wpDialog.showModal();
  $("#wp-name").focus();
}

$("#wp-new").addEventListener("click", () => openWaypointDialog());
wpDialog.querySelectorAll("[data-close]").forEach((b) => b.addEventListener("click", () => wpDialog.close()));

$("#wp-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const y = $("#wp-y").value.trim();
  const body = {
    name: $("#wp-name").value, color: wpColor, dim: $("#wp-dim").value,
    x: Number($("#wp-x").value), y: y === "" ? null : Number(y), z: Number($("#wp-z").value),
  };
  try {
    if (wpEditing) await api(S(`/waypoints/${encodeURIComponent(wpEditing.id)}`), { method: "PUT", body });
    else await api(S("/waypoints"), { method: "POST", body: { ...body, owner: $("#wp-owner").value || null } });
    wpDialog.close();
    toast(`Saved "${body.name.trim()}"`);
    loadWaypoints();
  } catch (err) {
    toast(err.message, true);
  }
});

// ---------- downloaded servers ----------

function renderInstalled() {
  const ul = $("#installed");
  ul.replaceChildren();
  if (!ui.installed.length) {
    ul.append(el("li", { className: "empty", textContent: "Nothing downloaded yet" }));
    return;
  }
  const s = ui.status || {};
  for (const v of ui.installed) {
    const running = s.state !== "stopped" && s.flavor === v.flavor && s.version === v.version;
    const selected = ui.settings && ui.settings.flavor === v.flavor && ui.settings.version === v.version;
    const build = v.build ? (v.flavor === "fabric" ? `loader ${v.build}` : `build ${v.build}`) : null;
    const meta = [build, `${v.size_mb} MB`, v.java ? `Java ${v.java}` : null].filter(Boolean).join(" · ");
    const del = el("button", {
      className: "icon-btn small danger", title: running ? "Running — stop the server first" : "Delete",
      ariaLabel: `Delete ${flavorLabel(v.flavor, v.version)}`, disabled: running,
    }, icon("trash"));
    del.addEventListener("click", () => deleteInstalled(v));
    let upd = null;
    if (v.flavor === "paper" || v.flavor === "fabric") {
      upd = el("button", {
        className: "icon-btn small", title: v.flavor === "paper" ? "Check for a newer Paper build" : "Check for a newer Fabric loader",
        ariaLabel: `Update ${flavorLabel(v.flavor, v.version)}`, disabled: !!s.task,
      }, icon("arrow-clockwise"));
      upd.addEventListener("click", () => updateInstalled(v));
    }
    ul.append(el("li", { className: selected ? "current" : "" },
      tag(FLAVOR_NAME[v.flavor], { paper: "info", fabric: "violet" }[v.flavor] || ""),
      el("div", { className: "grow" },
        el("div", { className: "title", textContent: v.version }),
        el("div", { className: "meta", textContent: meta })),
      missingJava(v.flavor, v.version) ? tag(`needs Java ${v.java}`, "warn", "warning") : null,
      running ? tag("running", "accent") : selected ? tag("selected", "accent") : null,
      upd, del));
  }
}

async function updateInstalled(v) {
  try {
    await api(S(`/versions/${v.flavor}/${encodeURIComponent(v.version)}/update`), { method: "POST" });
    toast(`Checking for updates of ${flavorLabel(v.flavor, v.version)}…`);
  } catch (err) {
    toast(err.message, true);
  }
}

async function deleteInstalled(v) {
  if (!(await ask({ title: `Delete ${flavorLabel(v.flavor, v.version)}?`, text: "The downloaded server jar is deleted. Worlds and backups are not affected.", ok: "Delete", danger: true, iconName: "trash" }))) return;
  try {
    await api(`/api/versions/${v.flavor}/${encodeURIComponent(v.version)}`, { method: "DELETE" });
    toast(`${flavorLabel(v.flavor, v.version)} deleted`);
    await loadState(false);
  } catch (err) {
    toast(err.message, true);
  }
}

// ---------- version selection ----------

const isInstalled = (f, v) => ui.installed.some((i) => i.flavor === f && i.version === v);

function rankOf(version) {
  return ui.versions.find((v) => v.id === version)?.rank
    ?? ui.installed.find((v) => v.version === version)?.rank
    ?? null;
}

// Fallback when Mojang's order is unknown: compare "1.21.4" / "26.1.2" numerically
function compareIds(a, b) {
  const pa = a.split(/[.\-]/).map(Number), pb = b.split(/[.\-]/).map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const x = pa[i] ?? 0, y = pb[i] ?? 0;
    if (Number.isNaN(x) || Number.isNaN(y)) return 0;
    if (x !== y) return x < y ? -1 : 1;
  }
  return 0;
}

/** -1 = older than the world (downgrade), 1 = newer (upgrade), 0 = same/unknown */
function directionVsWorld(version) {
  const w = ui.world;
  if (!w?.version || !version || version === w.version) return 0;
  const r = rankOf(version), wr = w.rank ?? rankOf(w.version);
  if (r != null && wr != null) return r > wr ? -1 : r < wr ? 1 : 0; // rank 0 = newest
  return compareIds(version, w.version);
}

function versionHints() {
  const { flavor, version } = ui.sel;
  const w = ui.world;
  const msgs = [];
  let level = "info";
  if (settingsDirty()) msgs.push("Not saved yet: press Save, or Start to apply and start.");
  if (!version) return { level, msgs };

  const dir = directionVsWorld(version);
  if (dir < 0) {
    level = "warn";
    msgs.push(`Downgrade: your world was last played on ${w.version}. Loading it with the older ${version} can break or reset parts of the world. Make a backup first.`);
  } else if (dir > 0) {
    msgs.push(`Upgrade from ${w.version} to ${version}. The world is converted on first start; going back later needs a backup.`);
  }
  if (w?.flavor && w.flavor !== flavor) {
    if (flavor === "paper") msgs.push("Switching to Paper: Paper moves the Nether and End into separate world folders automatically.");
    else if (w.flavor === "paper") msgs.push(`Switching to ${FLAVOR_NAME[flavor]}: Paper keeps the Nether and End in separate folders, which ${FLAVOR_NAME[flavor]} does not load without manual conversion.`);
    if (w.flavor === "fabric") msgs.push("Leaving Fabric: blocks and items from mods are removed when the world is loaded without them.");
    if (flavor === "fabric") msgs.push("Fabric downloads the Minecraft server and its libraries on the first start. Install mods on the Add-ons page.");
    msgs.push("A backup of the world is made automatically when you save.");
  }
  const java = missingJava(flavor, version);
  if (java) {
    level = "warn";
    msgs.push(`${version} needs Java ${java}, but only Java ${ui.java.join(", ")} is installed. ` +
              "The Docker image includes it; without Docker, install it first.");
  }
  if (!isInstalled(flavor, version)) msgs.push("This version will be downloaded on the next start.");
  return { level, msgs };
}

function renderHint() {
  const box = $("#version-hint");
  const { level, msgs } = versionHints();
  box.classList.toggle("hidden", !msgs.length);
  if (!msgs.length) return;
  box.className = `hint-box ${level}`;
  box.querySelector("use").setAttribute("href", `${ICONS}#ph-${level === "warn" ? "warning" : "info"}`);
  box.querySelector(".hint-text").replaceChildren(...msgs.map((m) => el("p", { textContent: m })));
}

/** Java major a version needs: exact for downloaded jars, otherwise Mojang's known thresholds. */
function javaNeeded(flavor, id) {
  const known = ui.installed.find((i) => i.flavor === flavor && i.version === id)?.java;
  if (known) return known;
  const [major, minor = 0, patch = 0] = id.split(/[.\-]/).map(Number);
  if (Number.isNaN(major)) return null;
  if (major >= 26) return 25;                       // 26.x
  if (major !== 1) return null;
  if (minor > 20 || (minor === 20 && patch >= 5)) return 21;
  if (minor >= 17) return 17;
  return 8;
}

/** Required Java major if no suitable runtime is installed, else null. */
function missingJava(flavor, id) {
  const need = javaNeeded(flavor, id);
  if (!need || !ui.java?.length) return null;
  return ui.java.some((v) => v >= need) ? null : need;
}

function versionTags(id, snapshot) {
  const tags = [];
  if (id === ui.latest) tags.push(tag("latest", "accent"));
  if (snapshot) tags.push(tag("snapshot"));
  if (isInstalled(ui.sel.flavor, id)) tags.push(tag("downloaded", "info", "check"));
  if (directionVsWorld(id) < 0) tags.push(tag("older than world", "warn", "warning"));
  const java = missingJava(ui.sel.flavor, id);
  if (java) tags.push(tag(`needs Java ${java}`, "warn", "warning"));
  return tags;
}

function renderToggle() {
  const value = $("#version-dd .dd-value");
  const v = ui.sel.version;
  value.textContent = v || "Choose a version";
  value.classList.toggle("placeholder", !v);
  const info = ui.versions.find((x) => x.id === v);
  $("#version-dd .dd-tags").replaceChildren(...(v ? versionTags(v, info?.snapshot) : []));
}

async function loadVersions() {
  const { flavor } = ui.sel;
  const sid = ui.sid;
  try {
    const data = await api(`/api/versions?flavor=${flavor}&snapshots=${$("#snapshots").checked}`);
    if (flavor !== ui.sel?.flavor || sid !== ui.sid) return; // switched while loading
    ui.versions = data.versions;
    ui.latest = data.latest;
  } catch (err) {
    // Offline: still allow choosing among already downloaded jars
    ui.versions = ui.installed.filter((i) => i.flavor === flavor).map((i) => ({ id: i.version, rank: i.rank }));
    ui.latest = null;
    toast(err.message, true);
  }
  renderToggle();
  renderHint();
  if (dd.isOpen()) dd.renderList();
}

// Custom dropdown with search and keyboard support
const dd = (() => {
  const root = $("#version-dd");
  const toggle = root.querySelector(".dd-toggle");
  const panel = root.querySelector(".dd-panel");
  const search = root.querySelector(".dd-search input");
  const list = root.querySelector(".dd-list");
  let items = [];
  let active = -1;

  const isOpen = () => root.classList.contains("open");

  function setActive(i) {
    const lis = list.querySelectorAll("li[data-id]");
    if (!lis.length) return;
    active = Math.max(0, Math.min(i, lis.length - 1));
    lis.forEach((li, n) => li.classList.toggle("active", n === active));
    lis[active].scrollIntoView({ block: "nearest" });
  }

  function renderList() {
    const q = search.value.trim().toLowerCase();
    items = ui.versions.filter((v) => !q || v.id.toLowerCase().includes(q));
    // An exact match goes first, so typing "1.21.1" + Enter doesn't pick "1.21.11"
    const exact = items.findIndex((v) => v.id.toLowerCase() === q);
    if (exact > 0) items.unshift(...items.splice(exact, 1));
    list.replaceChildren();
    if (!items.length) {
      list.append(el("li", { className: "empty", textContent: q ? "No matching version" : "No versions available" }));
      active = -1;
      return;
    }
    items.forEach((v) => {
      const li = el("li", {
        role: "option", ariaSelected: String(v.id === ui.sel.version),
        className: directionVsWorld(v.id) < 0 ? "down" : "", dataset: { id: v.id },
      }, el("span", { className: "id", textContent: v.id }),
        el("span", { className: "tags" }, ...versionTags(v.id, v.snapshot)),
        icon("check", "i sel"));
      li.addEventListener("click", () => choose(v.id));
      li.addEventListener("mousemove", () => setActive(items.indexOf(v)));
      list.append(li);
    });
    const cur = q ? -1 : items.findIndex((v) => v.id === ui.sel.version);
    setActive(cur >= 0 ? cur : 0);
  }

  function open() {
    root.classList.add("open");
    panel.classList.remove("hidden");
    toggle.setAttribute("aria-expanded", "true");
    search.value = "";
    renderList();
    search.focus();
  }

  function close(refocus = false) {
    root.classList.remove("open");
    panel.classList.add("hidden");
    toggle.setAttribute("aria-expanded", "false");
    if (refocus) toggle.focus();
  }

  function choose(id) {
    ui.sel.version = id;
    close(true);
    renderToggle();
    renderHint();
  }

  toggle.addEventListener("click", () => (isOpen() ? close() : open()));
  toggle.addEventListener("keydown", (e) => {
    if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key) && !isOpen()) { e.preventDefault(); open(); }
  });
  search.addEventListener("input", renderList);
  search.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") { e.preventDefault(); setActive(active + 1); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setActive(active - 1); }
    else if (e.key === "Enter") { e.preventDefault(); if (items[active]) choose(items[active].id); }
    else if (e.key === "Escape") { e.preventDefault(); close(true); }
    else if (e.key === "Tab") close();
  });
  document.addEventListener("mousedown", (e) => { if (isOpen() && !root.contains(e.target)) close(); });

  return { isOpen, renderList };
})();

$$('input[name="flavor"]').forEach((r) => r.addEventListener("change", () => {
  ui.sel.flavor = r.value;
  ui.versions = [];
  renderToggle();
  loadVersions().then(() => {
    // Keep the version if this flavor offers it, otherwise ask for a new choice
    if (ui.sel.version && !ui.versions.some((v) => v.id === ui.sel.version)) {
      ui.sel.version = null;
      renderToggle();
      renderHint();
    }
  });
}));
$("#snapshots").addEventListener("change", loadVersions);

/** True if the Server page differs from what is saved on the server. */
function settingsDirty() {
  const s = ui.settings;
  if (!s || !ui.sel) return false;
  return ui.sel.flavor !== s.flavor
    || (ui.sel.version || "") !== (s.version || "")
    || $("#min-ram").value.trim().toUpperCase() !== s.min_ram
    || $("#max-ram").value.trim().toUpperCase() !== s.max_ram;
}

/** Save the Server page. Returns false if cancelled or failed. */
async function saveSettings() {
  const { flavor, version } = ui.sel;
  if (version && version !== ui.settings?.version && directionVsWorld(version) < 0 &&
      !(await ask({ title: "Downgrade warning", text: `Your world was last played on ${ui.world.version}.\n` +
               `Running it on the older ${version} can corrupt or reset parts of it. Make a backup first.`,
               ok: "Continue anyway", danger: true, iconName: "warning" }))) {
    return false;
  }
  try {
    await api(S("/settings"), {
      method: "POST",
      body: { flavor, version: version || "", min_ram: $("#min-ram").value, max_ram: $("#max-ram").value },
    });
    await Promise.all([loadState(false), loadServers()]);
    // Type/version may have changed: mods vs. plugins, other search results
    ui.addons = ui.addonUpdates = null;
    $("#mr-results").replaceChildren();
    return true;
  } catch (err) {
    toast(err.message, true);
    return false;
  }
}

$("#settings-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (await saveSettings()) {
    toast("Settings saved" + (ui.status?.state !== "stopped" ? " — RAM changes apply after a restart" : ""));
  }
});
$("#min-ram").addEventListener("input", () => renderHint());
$("#max-ram").addEventListener("input", () => renderHint());

$("#install-btn").addEventListener("click", async () => {
  const { flavor, version } = ui.sel;
  if (!version) return toast("Choose a version first", true);
  try {
    await api(S("/versions/install"), { method: "POST", body: { flavor, version } });
    toast(`Downloading ${flavorLabel(flavor, version)}…`);
  } catch (err) {
    toast(err.message, true);
  }
});

// ---------- game settings (server.properties) ----------

const propsDialog = $("#props-dialog");

function propInputs() {
  return [...propsDialog.querySelectorAll("[data-key]")];
}

$("#props-btn").addEventListener("click", async () => {
  try {
    const { values } = await api(S("/properties"));
    for (const node of propInputs()) {
      const v = values[node.dataset.key];
      if (node.classList.contains("segmented")) {
        node.querySelectorAll("input").forEach((r) => { r.checked = r.value === v; });
      } else if (node.type === "checkbox") {
        node.checked = !!v;
      } else {
        node.value = v;
      }
    }
    $("#props-note").textContent = ui.status?.state !== "stopped" ? "Changes apply after a restart." : "";
    propsDialog.showModal();
  } catch (err) {
    toast(err.message, true);
  }
});

propsDialog.querySelectorAll("[data-close]").forEach((b) => b.addEventListener("click", () => propsDialog.close()));

$("#props-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const values = {};
  for (const node of propInputs()) {
    const key = node.dataset.key;
    if (node.classList.contains("segmented")) {
      const checked = node.querySelector("input:checked");
      if (checked) values[key] = checked.value;
    } else if (node.type === "checkbox") {
      values[key] = node.checked;
    } else if (node.type === "number") {
      values[key] = Number(node.value);
    } else {
      values[key] = node.value;
    }
  }
  try {
    await api(S("/properties"), { method: "POST", body: values });
    propsDialog.close();
    toast("Game settings saved" + (ui.status?.state !== "stopped" ? " — restart to apply" : ""));
  } catch (err) {
    toast(err.message, true);
  }
});

// ---------- new world ----------

const worldDialog = $("#world-dialog");

$("#new-world-btn").addEventListener("click", async () => {
  if (ui.status?.state !== "stopped") return toast("Stop the server before creating a new world", true);
  $("#world-seed").value = "";
  // Pack choice: everything known (library + current world), preselected = active in the current world
  let packs = [];
  try {
    packs = (await api(S("/datapacks"))).packs.filter((p) => p.valid);
  } catch (err) {
    toast(err.message, true);
  }
  $("#world-packs-wrap").classList.toggle("hidden", !packs.length);
  $("#world-packs").replaceChildren(...packs.map((p) => el("label", { className: "check" },
    el("input", { type: "checkbox", value: p.name, checked: p.installed && p.status !== "disabled" }),
    el("span", { className: "box" }, icon("check")),
    el("span", { textContent: (p.title || p.name.replace(/\.zip$/i, "")) + (p.in_library ? "" : " (copied to the library)") }))));
  worldDialog.showModal();
});
worldDialog.querySelectorAll("[data-close]").forEach((b) => b.addEventListener("click", () => worldDialog.close()));

$("#world-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  try {
    const datapacks = [...$("#world-packs").querySelectorAll("input:checked")].map((i) => i.value);
    await api(S("/world/new"), { method: "POST", body: { seed: $("#world-seed").value, datapacks } });
    worldDialog.close();
    toast("Archiving the old world…");
  } catch (err) {
    toast(err.message, true);
  }
});

// ---------- actions ----------

const CONFIRM = {
  stop: { title: "Stop the server?", ok: "Stop", iconName: "stop" },
  restart: { title: "Restart the server?", ok: "Restart", iconName: "arrow-clockwise" },
  kill: { title: "Force kill the server process?", text: "Unsaved world data may be lost.", ok: "Kill", danger: true, iconName: "lightning" },
};

$$("[data-action]").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const action = btn.dataset.action;
    if (CONFIRM[action] && !(await ask({ ...CONFIRM[action], text: `${ui.server?.name || "Server"}\n${CONFIRM[action].text || ""}` }))) return;
    // Start what is selected on the Server page, even if "Save" wasn't pressed
    if (action === "start" && settingsDirty() && !(await saveSettings())) return;
    const countdown = Number($("#countdown").value);
    const body = ["stop", "restart"].includes(action) ? { countdown } : undefined;
    const path = action === "backup" ? S("/backups") : S(`/${action}`);
    try {
      await api(path, { method: "POST", body });
    } catch (err) {
      toast(err.message, true);
    }
  });
});

$("#eula-accept").addEventListener("click", async () => {
  try {
    await api(S("/eula"), { method: "POST" });
    toast("EULA accepted — you can start the server now");
  } catch (err) {
    toast(err.message, true);
  }
});

$("#command-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const input = $("#command");
  const cmd = input.value.trim();
  if (!cmd) return;
  try {
    await api(S("/command"), { method: "POST", body: { command: cmd } });
    if (ui.history[ui.history.length - 1] !== cmd) ui.history.push(cmd);
    ui.historyPos = ui.history.length;
    input.value = "";
  } catch (err) {
    toast(err.message, true);
  }
});

$("#command").addEventListener("keydown", (e) => {
  if (e.key === "ArrowUp" && ui.historyPos > 0) ui.historyPos--;
  else if (e.key === "ArrowDown" && ui.historyPos < ui.history.length) ui.historyPos++;
  else return;
  e.preventDefault();
  e.target.value = ui.history[ui.historyPos] ?? "";
});

$("#say-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const msg = $("#say").value.trim();
  if (!msg) return;
  try {
    await api(S("/command"), { method: "POST", body: { command: `say ${msg}` } });
    $("#say").value = "";
  } catch (err) {
    toast(err.message, true);
  }
});

// ---------- live connection ----------

function setLive(on) {
  $("#ws-indicator").classList.toggle("on", on);
  $("#ws-text").textContent = on ? "Live" : "Reconnecting…";
}

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/ws`);
  ui.ws = ws;

  ws.onopen = () => {
    ui.wsRetry = 1000;
    setLive(true);
  };
  ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.server && msg.server !== ui.sid) {
      // Another server: only its state matters here (dot in the sidebar)
      if (msg.type === "status") updateServerEntry(msg.status);
      return;
    }
    switch (msg.type) {
      case "log":
        appendLog([msg.line]);
        if (/Backup saved|downloaded \(|updated to (build|loader)|added to downloaded servers|restored\.$|New world prepared/.test(msg.line)) loadState(false);
        // OP/whitelist changed via console or in-game: re-read once the server has written the file
        if (/Made \w+ (a server operator|no longer a server operator)|(Added|Removed) \w+ (to|from) the whitelist|Whitelist is now turned (on|off)|Banned \w+|Unbanned \w+/i.test(msg.line)) {
          setTimeout(loadAccess, 600);
        }
        break;
      case "chat": appendChat(msg.entry); break;
      case "status": {
        const was = ui.status?.state;
        renderStatus(msg.status);
        if (was === "starting" && msg.status.state === "running") loadState(false); // world version updated
        break;
      }
      case "stats": renderStats(msg.stats); break;
    }
  };
  ws.onclose = (ev) => {
    setLive(false);
    if (ws !== ui.ws) return; // superseded or logged out
    if (ev.code === 4401) return showLogin();
    scheduleReconnect();
  };
}

function scheduleReconnect() {
  setTimeout(async () => {
    if ($("#app").classList.contains("hidden")) return; // logged out meanwhile
    try {
      await loadServers();
      await loadState(true); // resync everything missed while disconnected
      connect();
    } catch {
      scheduleReconnect(); // panel unreachable (e.g. container restarting): keep trying
    }
  }, ui.wsRetry);
  ui.wsRetry = Math.min(ui.wsRetry * 2, 15000);
}

async function loadState(full) {
  const sid = ui.sid;
  const data = await api(S("/state"));
  if (sid !== ui.sid) return; // switched to another server meanwhile
  const firstLoad = !ui.sel;
  ui.server = data.server;
  ui.settings = data.settings;
  ui.world = data.world;
  ui.installed = data.installed;
  ui.java = Object.keys(data.java).map(Number);
  if (firstLoad) {
    ui.sel = { flavor: data.settings.flavor, version: data.settings.version || null };
    const radio = document.querySelector(`input[name="flavor"][value="${ui.sel.flavor}"]`);
    if (radio) radio.checked = true;
  }
  renderServerInfo();
  renderAddonTabLabel();
  renderStatus(data.status);
  renderBackups(data.backups, data.backup_keep, data.backup_interval);
  $("#world-version").textContent = data.world.version
    ? flavorLabel(data.world.flavor || data.settings.flavor, data.world.version) : "not started yet";
  const javas = Object.keys(data.java);
  $("#java-list").textContent = javas.length ? `Java ${javas.join(" · ")}` : "no Java found";
  if (full || firstLoad) {
    resetLog(data.log);
    resetChat(data.chat);
    $("#min-ram").value = data.settings.min_ram;
    $("#max-ram").value = data.settings.max_ram;
    $("#map-enabled").checked = data.settings.map_enabled;
    loadAccess();
  }
  loadDatapacks();
  loadHistory();
  if (firstLoad || full) await loadVersions();
  else { renderToggle(); renderHint(); }
}

async function boot() {
  await loadServers();
  $("#login").classList.add("hidden");
  $("#app").classList.remove("hidden");
  ui.sid = null; // force a full load of the server in the URL
  await route();
  if (!ui.ws) connect();
}

applyLogFilters();
showPlayerTab("online");
boot().catch((err) => {
  // 401 already shows the login screen; anything else (panel unreachable) too, with the error
  if ($("#login").classList.contains("hidden")) {
    showLogin();
    $("#login-error").textContent = err.message;
  }
});
