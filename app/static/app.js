"use strict";

const $ = (sel) => document.querySelector(sel);
const MAX_LINES = 1000;
const ICONS = "/static/icons.svg";
const FLAVOR_NAME = { vanilla: "Vanilla", paper: "Paper" };

const ui = {
  status: null,
  settings: null,   // saved settings from the server
  world: {},        // { version, flavor, rank } that last ran the world
  installed: [],    // [{ flavor, version, build, size_mb, rank }]
  sel: null,        // unsaved selection { flavor, version }
  versions: [],     // version list for the selected flavor
  latest: null,
  history: [],
  historyPos: 0,
  ws: null,
  wsRetry: 1000,
};

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
  return h ? `up ${h}h ${m}m` : `up ${m}m`;
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

function tag(text, cls = "", iconName = null) {
  return el("span", { className: `tag ${cls}` }, iconName ? icon(iconName) : null, text);
}

const flavorLabel = (f, v) => `${FLAVOR_NAME[f] || f} ${v}`;

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

// ---------- console & chat ----------

function lineClass(line) {
  if (line.startsWith("[GUI]")) return "gui";
  if (/(\/| )(ERROR|FATAL)\]|Exception/.test(line)) return "err";
  if (/(\/| )WARN\]/.test(line)) return "warn";
  if (/\]: (\[Not Secure\] )?<\w+>/.test(line)) return "chat";
  return "";
}

function appendLog(lines) {
  const con = $("#console");
  const frag = document.createDocumentFragment();
  for (const line of lines) frag.append(el("span", { className: lineClass(line), textContent: line + "\n" }));
  con.append(frag);
  while (con.childElementCount > MAX_LINES) con.firstElementChild.remove();
  if ($("#autoscroll").checked) con.scrollTop = con.scrollHeight;
}

function appendChat(entry) {
  const chat = $("#chat");
  chat.querySelector(".empty")?.remove();
  chat.append(el("div", {}, el("time", { textContent: entry.time }), entry.text));
  while (chat.childElementCount > 300) chat.firstElementChild.remove();
  chat.scrollTop = chat.scrollHeight;
}

// ---------- status / stats ----------

function renderStatus(s) {
  ui.status = s;
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
  const mapUrl = s.map_port ? `http://${location.hostname}:${s.map_port}/` : null;
  for (const a of [$("#map-link"), $("#map-open")]) {
    a.classList.toggle("hidden", !mapUrl);
    if (mapUrl) a.href = mapUrl;
  }
  $("#map-render").classList.toggle("hidden", !ui.settings?.map_enabled);

  const alive = s.state !== "stopped";
  const busy = !!s.task;
  const btn = (a) => document.querySelector(`[data-action="${a}"]`);
  btn("start").disabled = alive || busy;
  btn("stop").disabled = !alive || busy;
  btn("restart").disabled = !alive || busy;
  btn("backup").disabled = busy;
  btn("kill").disabled = !alive;
  btn("cancel").disabled = !busy || !["stop", "restart"].includes(s.task);
  $("#install-btn").disabled = busy;

  $("#player-count").textContent = s.players.length;
  const list = $("#players");
  list.replaceChildren();
  if (!s.players.length) {
    list.append(el("li", { className: "empty", textContent: alive ? "Nobody online" : "Server offline" }));
  }
  for (const name of s.players) {
    list.append(el("li", {}, el("span", { className: "avatar", textContent: name[0].toUpperCase() }),
      el("span", { className: "grow", textContent: name }),
      playerActions(name)));
  }
  renderInstalled(); // running marker / delete buttons depend on status
  if (ui.datapacks) renderDatapacks(ui.datapacks);
  renderAccess();    // "recently rejected" comes with the status
}

// ---------- whitelist & operators ----------

const lower = (n) => n.toLowerCase();
const onWhitelist = (n) => (ui.access?.whitelist || []).some((w) => lower(w) === lower(n));
const isOp = (n) => (ui.access?.ops || []).some((o) => lower(o.name) === lower(n));

/** Quick toggles for a player row: whitelist + operator. */
function playerActions(name) {
  const wl = onWhitelist(name), op = isOp(name);
  const mk = (iconName, on, title, kind) => {
    const b = el("button", { className: `icon-btn small${on ? " on" : ""}`, title, ariaLabel: title }, icon(iconName));
    b.addEventListener("click", () => changeAccess(kind, on ? "remove" : "add", name));
    return b;
  };
  const kick = el("button", { className: "icon-btn small danger", title: `Kick ${name}`, ariaLabel: `Kick ${name}` }, icon("sign-out"));
  kick.addEventListener("click", async () => {
    if (!confirm(`Kick ${name} from the server?`)) return;
    try {
      await api("/api/command", { method: "POST", body: { command: `kick ${name}` } });
    } catch (err) {
      toast(err.message, true);
    }
  });
  return el("span", { className: "row-actions" },
    mk("list-checks", wl, wl ? `Remove ${name} from whitelist` : `Add ${name} to whitelist`, "whitelist"),
    mk("crown-simple", op, op ? `Remove operator rights from ${name}` : `Make ${name} operator`, "ops"),
    kick);
}

$("#map-render").addEventListener("click", async () => {
  try {
    await api("/api/map/render", { method: "POST" });
    toast(ui.status?.state === "running" ? "Saving the world, then updating the map…" : "Updating the map…");
  } catch (err) {
    toast(err.message, true);
  }
});

function removeButton(title, onClick) {
  const b = el("button", { className: "icon-btn small danger", title, ariaLabel: title }, icon("x"));
  b.addEventListener("click", onClick);
  return b;
}

function renderAccess() {
  const a = ui.access;
  if (!a) return;
  $("#wl-enabled").checked = a.whitelist_enabled;

  const wl = $("#whitelist");
  wl.replaceChildren();
  // Players the whitelist just turned away, with a one-click "add"
  for (const name of (ui.status?.rejected || []).filter((n) => !onWhitelist(n))) {
    const add = el("button", { className: "icon-btn small on", title: `Add ${name} to whitelist`, ariaLabel: `Add ${name} to whitelist` }, icon("user-plus"));
    add.addEventListener("click", () => changeAccess("whitelist", "add", name));
    wl.append(el("li", { className: "current" },
      el("span", { className: "avatar", textContent: name[0].toUpperCase() }),
      el("div", { className: "grow" }, el("div", { textContent: name }),
        el("div", { className: "meta", textContent: "just tried to join — not whitelisted" })),
      add));
  }
  if (!a.whitelist.length && !wl.childElementCount) {
    wl.append(el("li", { className: "empty", textContent: "Nobody on the whitelist yet" }));
  }
  for (const name of a.whitelist) {
    wl.append(el("li", {},
      el("span", { className: "avatar", textContent: name[0].toUpperCase() }),
      el("span", { className: "grow", textContent: name }),
      removeButton(`Remove ${name} from whitelist`, () => changeAccess("whitelist", "remove", name))));
  }

  const ops = $("#ops");
  ops.replaceChildren();
  if (!a.ops.length) ops.append(el("li", { className: "empty", textContent: "No operators yet" }));
  for (const o of a.ops) {
    ops.append(el("li", {},
      el("span", { className: "avatar", textContent: o.name[0].toUpperCase() }),
      el("div", { className: "grow" }, el("div", { textContent: o.name }),
        el("div", { className: "meta", textContent: `permission level ${o.level}` })),
      removeButton(`Remove operator rights from ${o.name}`, () => changeAccess("ops", "remove", o.name))));
  }
}

async function loadAccess() {
  try {
    ui.access = await api("/api/access");
    renderAccess();
    if (ui.status) renderStatus(ui.status); // refresh toggles in the online list
  } catch { /* shown elsewhere */ }
}

async function changeAccess(kind, action, name) {
  if (kind === "ops" && action === "add" &&
      !confirm(`Make ${name} an operator?\n\nOperators can use every command, including /stop and /op.`)) return;
  try {
    const res = await api(`/api/access/${kind}`, { method: "POST", body: { action, name } });
    ui.access = res;
    toast(res.message);
    renderAccess();
    if (ui.status) renderStatus(ui.status);
  } catch (err) {
    toast(err.message, true);
  }
}

document.querySelectorAll(".add-form").forEach((form) => form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const input = form.querySelector("input");
  const name = input.value.trim();
  if (!name) return;
  await changeAccess(form.dataset.kind, "add", name);
  if (ui.access && (form.dataset.kind === "ops" ? isOp(name) : onWhitelist(name))) input.value = "";
}));

$("#wl-enabled").addEventListener("change", async (e) => {
  try {
    ui.access = await api("/api/access/whitelist/enabled", { method: "POST", body: { enabled: e.target.checked } });
    toast(`Whitelist ${ui.access.whitelist_enabled ? "enabled" : "disabled"}`);
    renderAccess();
  } catch (err) {
    e.target.checked = !e.target.checked;
    toast(err.message, true);
  }
});

document.querySelectorAll('input[name="ptab"]').forEach((r) => r.addEventListener("change", () => {
  document.querySelectorAll(".players-card .tab-panel").forEach((p) => p.classList.toggle("hidden", p.dataset.tab !== r.value));
  if (r.value !== "online") loadAccess();
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
  if (d.experiments?.enabled.length) {
    ul.append(el("li", { className: "current", title: "Built-in experimental packs, chosen when the world was created" },
      tag("experiments", "warn"),
      el("div", { className: "grow meta", textContent: d.experiments.enabled.join(", ") })));
  }
  if (!d.packs.length) {
    ul.append(el("li", { className: "empty", textContent: "No data packs — add a .zip" }));
    return;
  }
  const alive = ui.status && ui.status.state !== "stopped";
  for (const p of d.packs) {
    const label = p.name.replace(/\.zip$/i, "");
    let toggle = null;
    if (p.installed) {
      const box = el("input", { type: "checkbox", checked: p.status !== "disabled", disabled: !p.valid });
      box.addEventListener("change", () => toggleDatapack(p, box));
      toggle = el("label", { className: "check", title: p.status !== "disabled" ? "Disable in this world" : "Enable in this world" },
        box, el("span", { className: "box" }, icon("check")));
    }
    const meta = [PACK_STATUS[p.status], p.in_library ? null : "only in this world", p.description, `${p.size_kb} KB`]
      .filter(Boolean).join(" · ");
    ul.append(el("li", { className: p.installed ? "" : "inactive", title: p.description },
      toggle,
      el("div", { className: "grow" }, el("div", { textContent: label }), el("div", { className: "meta", textContent: meta })),
      p.valid ? null : tag("no pack.mcmeta", "warn", "warning"),
      p.outdated ? tag("update", "info") : null,
      el("span", { className: "row-actions" },
        p.in_library && (!p.installed || p.outdated)
          ? packButton("download-simple", p.outdated ? `Update ${label} in this world from the library` : `Install ${label} in this world`,
              () => packAction(p, "install", "POST", p.outdated ? "updated" : "installed"))
          : null,
        p.in_library ? null
          : packButton("archive", `Copy ${label} to the library`, () => packAction(p, "adopt", "POST", "copied to the library")),
        p.installed
          ? packButton("x", alive ? "Stop the server to remove it from the world" : `Remove ${label} from this world`,
              () => removeDatapack(p), { danger: true, disabled: alive })
          : null,
        p.in_library ? packButton("trash", `Delete ${label} from the library`, () => deleteDatapack(p), { danger: true }) : null)));
  }
}

async function loadDatapacks() {
  try {
    renderDatapacks(await api("/api/datapacks"));
  } catch (err) {
    toast(err.message, true);
  }
}

async function packAction(p, action, method, done) {
  try {
    renderDatapacks(await api(`/api/datapacks/${encodeURIComponent(p.name)}/${action}`, { method }));
    toast(`${p.name} ${done}`);
  } catch (err) {
    toast(err.message, true);
  }
}

async function toggleDatapack(p, box) {
  const enabled = box.checked;
  box.disabled = true;
  try {
    renderDatapacks(await api(`/api/datapacks/${encodeURIComponent(p.name)}/enabled`, { method: "POST", body: { enabled } }));
    toast(`${p.name} ${enabled ? "enabled" : "disabled"}`);
  } catch (err) {
    box.checked = !enabled;
    box.disabled = false;
    toast(err.message, true);
  }
}

function removeDatapack(p) {
  const note = p.in_library ? "It stays in the library." : "It is NOT in the library, so it will be gone (copy it to the library first to keep it).";
  if (confirm(`Remove ${p.name} from this world?\n${note}`)) packAction(p, "world", "DELETE", "removed from the world");
}

function deleteDatapack(p) {
  const note = p.installed ? "The copy in the current world stays." : "";
  if (confirm(`Delete ${p.name} from the library?\n${note}`)) packAction(p, "library", "DELETE", "deleted from the library");
}

$("#datapack-file").addEventListener("change", async (e) => {
  const file = e.target.files[0];
  e.target.value = "";
  if (!file) return;
  toast(`Uploading ${file.name}…`);
  try {
    const res = await fetch(`/api/datapacks/upload?name=${encodeURIComponent(file.name)}`, {
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
});

function renderStats(st) {
  const mc = st.minecraft;
  $("#s-mc-state").textContent = mc ? "" : "· offline";
  $("#s-mc-cpu").textContent = mc ? `${mc.cpu.toFixed(1)} %` : "–";
  $("#b-mc-cpu").style.width = mc ? `${Math.min(mc.cpu, 100)}%` : "0";
  // RAM against the configured -Xmx (the JVM may use a bit more than the heap)
  $("#s-mc-ram").textContent = mc ? (mc.ram_max ? `${fmtMB(mc.ram)} / ${fmtMB(mc.ram_max)}` : fmtMB(mc.ram)) : "–";
  $("#b-mc-ram").style.width = mc?.ram_max ? `${Math.min((mc.ram / mc.ram_max) * 100, 100)}%` : "0";
  $("#s-panel").textContent = `${st.panel.cpu.toFixed(1)} % · ${fmtMB(st.panel.ram)}`;
  $("#net-row").classList.toggle("hidden", !st.net);
  if (st.net) $("#s-net").textContent = `${st.net.up} / ${st.net.down} KB/s`;
  if (ui.status) $("#uptime-label").textContent = fmtDuration(st.uptime);
}

// ---------- backups ----------

function renderBackups(list, keep, interval) {
  if (keep != null) {
    $("#backup-keep").textContent = (interval ? `every ${interval} h · ` : "") + `keeps ${keep}`;
  }
  const ul = $("#backups");
  ul.replaceChildren();
  if (!list.length) ul.append(el("li", { className: "empty", textContent: "No backups yet" }));
  for (const b of list) {
    const link = el("a", {
      className: "icon-btn small", href: `/api/backups/${encodeURIComponent(b.name)}`,
      title: "Download", ariaLabel: `Download backup ${b.created}`,
    }, icon("download-simple"));
    const restore = el("button", {
      className: "icon-btn small", title: "Restore this backup", ariaLabel: `Restore backup ${b.created}`,
    }, icon("clock-counter-clockwise"));
    restore.addEventListener("click", () => restoreBackup(b));
    const meta = `${b.size_mb} MB` + (b.note ? ` · ${b.note}` : "");
    ul.append(el("li", { className: b.archive ? "current" : "" },
      el("div", { className: "grow" },
        el("div", { textContent: b.created.replace("T", "  ") }),
        el("div", { className: "meta", textContent: meta })),
      b.archive ? tag("archive", "accent", "archive") : null,
      restore, link));
  }
}

async function restoreBackup(b) {
  if (ui.status?.state !== "stopped") return toast("Stop the server before restoring a backup", true);
  if (!confirm(`Restore the backup from ${b.created.replace("T", " ")}?\n\n` +
               "The current world is replaced. It is saved as a new backup first, so you can go back.")) return;
  try {
    await api(`/api/backups/${encodeURIComponent(b.name)}/restore`, { method: "POST" });
    toast("Restoring…");
  } catch (err) {
    toast(err.message, true);
  }
}

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
    const meta = [v.build ? `build ${v.build}` : null, `${v.size_mb} MB`, v.java ? `Java ${v.java}` : null]
      .filter(Boolean).join(" · ");
    const del = el("button", {
      className: "icon-btn small danger", title: running ? "Running — stop the server first" : "Delete",
      ariaLabel: `Delete ${flavorLabel(v.flavor, v.version)}`, disabled: running,
    }, icon("trash"));
    del.addEventListener("click", () => deleteInstalled(v));
    let upd = null;
    if (v.flavor === "paper") {
      upd = el("button", {
        className: "icon-btn small", title: "Check for a newer Paper build",
        ariaLabel: `Update ${flavorLabel(v.flavor, v.version)}`, disabled: !!s.task,
      }, icon("arrow-clockwise"));
      upd.addEventListener("click", () => updateInstalled(v));
    }
    ul.append(el("li", { className: selected ? "current" : "" },
      tag(FLAVOR_NAME[v.flavor], v.flavor === "paper" ? "info" : ""),
      el("div", { className: "grow" },
        el("div", { textContent: v.version }),
        el("div", { className: "meta", textContent: meta })),
      missingJava(v.flavor, v.version) ? tag(`needs Java ${v.java}`, "warn", "warning") : null,
      running ? tag("running", "accent") : selected ? tag("selected", "accent") : null,
      upd, del));
  }
}

async function updateInstalled(v) {
  try {
    await api(`/api/versions/paper/${encodeURIComponent(v.version)}/update`, { method: "POST" });
    toast(`Checking for a newer build of ${flavorLabel(v.flavor, v.version)}…`);
  } catch (err) {
    toast(err.message, true);
  }
}

async function deleteInstalled(v) {
  if (!confirm(`Delete the downloaded server ${flavorLabel(v.flavor, v.version)}?\nWorlds and backups are not affected.`)) return;
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
    msgs.push(flavor === "paper"
      ? "Switching to Paper: Paper moves the Nether and End into separate world folders automatically."
      : "Switching to Vanilla: Paper keeps the Nether and End in separate folders, which Vanilla does not load without manual conversion.");
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
  try {
    const data = await api(`/api/versions?flavor=${flavor}&snapshots=${$("#snapshots").checked}`);
    if (flavor !== ui.sel.flavor) return; // flavor switched while loading
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

document.querySelectorAll('input[name="flavor"]').forEach((r) => r.addEventListener("change", () => {
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

/** True if the Server card differs from what is saved on the server. */
function settingsDirty() {
  const s = ui.settings;
  if (!s || !ui.sel) return false;
  return ui.sel.flavor !== s.flavor
    || (ui.sel.version || "") !== (s.version || "")
    || $("#min-ram").value.trim().toUpperCase() !== s.min_ram
    || $("#max-ram").value.trim().toUpperCase() !== s.max_ram
    || $("#map-enabled").checked !== s.map_enabled;
}

/** Save the Server card. Returns false if cancelled or failed. */
async function saveSettings() {
  const { flavor, version } = ui.sel;
  if (version && version !== ui.settings?.version && directionVsWorld(version) < 0 &&
      !confirm(`Downgrade warning\n\nYour world was last played on ${ui.world.version}. ` +
               `Running it on the older ${version} can corrupt or reset parts of it.\n\nMake a backup first. Continue anyway?`)) {
    return false;
  }
  try {
    await api("/api/settings", {
      method: "POST",
      body: {
        flavor, version: version || "", min_ram: $("#min-ram").value, max_ram: $("#max-ram").value,
        map_enabled: $("#map-enabled").checked,
      },
    });
    await loadState(false);
    return true;
  } catch (err) {
    toast(err.message, true);
    return false;
  }
}

$("#settings-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (await saveSettings()) {
    toast("Settings saved" + (ui.status?.state !== "stopped" ? " — RAM and map changes apply after a restart" : ""));
  }
});
$("#map-enabled").addEventListener("change", () => renderHint());
$("#min-ram").addEventListener("input", () => renderHint());
$("#max-ram").addEventListener("input", () => renderHint());

$("#install-btn").addEventListener("click", async () => {
  const { flavor, version } = ui.sel;
  if (!version) return toast("Choose a version first", true);
  try {
    await api("/api/versions/install", { method: "POST", body: { flavor, version } });
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
    const { values } = await api("/api/properties");
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
    await api("/api/properties", { method: "POST", body: values });
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
  let packs = [], exp = { available: [], enabled: [] };
  try {
    const d = await api("/api/datapacks");
    packs = d.packs.filter((p) => p.valid);
    exp = d.experiments;
  } catch (err) {
    toast(err.message, true);
  }
  // Experiments: unchecked by default, even if the old world had them
  $("#world-exp-wrap").open = false;
  $("#world-exp").replaceChildren(...exp.available.map((id) => el("label", { className: "check" },
    el("input", { type: "checkbox", value: id }),
    el("span", { className: "box" }, icon("check")),
    el("span", { textContent: id + (exp.enabled.includes(id) ? " (on in the current world)" : "") }))));
  $("#world-packs-wrap").classList.toggle("hidden", !packs.length);
  $("#world-packs").replaceChildren(...packs.map((p) => el("label", { className: "check" },
    el("input", { type: "checkbox", value: p.name, checked: p.installed && p.status !== "disabled" }),
    el("span", { className: "box" }, icon("check")),
    el("span", { textContent: p.name.replace(/\.zip$/i, "") + (p.in_library ? "" : " (copied to the library)") }))));
  worldDialog.showModal();
});
worldDialog.querySelectorAll("[data-close]").forEach((b) => b.addEventListener("click", () => worldDialog.close()));

$("#world-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  try {
    const datapacks = [...$("#world-packs").querySelectorAll("input:checked")].map((i) => i.value);
    const experiments = [...$("#world-exp").querySelectorAll("input:checked")].map((i) => i.value);
    await api("/api/world/new", { method: "POST", body: { seed: $("#world-seed").value, datapacks, experiments } });
    worldDialog.close();
    toast("Archiving the old world…");
  } catch (err) {
    toast(err.message, true);
  }
});

// ---------- actions ----------

const CONFIRM = {
  stop: "Stop the server?",
  restart: "Restart the server?",
  kill: "Force kill the server process? Unsaved world data may be lost.",
};

document.querySelectorAll("[data-action]").forEach((btn) => {
  btn.addEventListener("click", async () => {
    const action = btn.dataset.action;
    if (CONFIRM[action] && !confirm(CONFIRM[action])) return;
    // Start what is selected in the Server card, even if "Save" wasn't pressed
    if (action === "start" && settingsDirty() && !(await saveSettings())) return;
    const countdown = Number(document.querySelector('input[name="countdown"]:checked').value);
    const body = ["stop", "restart"].includes(action) ? { countdown } : undefined;
    const path = action === "backup" ? "/api/backups" : `/api/${action}`;
    try {
      await api(path, { method: "POST", body });
    } catch (err) {
      toast(err.message, true);
    }
  });
});

$("#eula-accept").addEventListener("click", async () => {
  try {
    await api("/api/eula", { method: "POST" });
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
    await api("/api/command", { method: "POST", body: { command: cmd } });
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
    await api("/api/command", { method: "POST", body: { command: `say ${msg}` } });
    $("#say").value = "";
  } catch (err) {
    toast(err.message, true);
  }
});

// ---------- live connection ----------

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/ws`);
  ui.ws = ws;

  ws.onopen = () => {
    ui.wsRetry = 1000;
    $("#ws-indicator").classList.add("on");
  };
  ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data);
    switch (msg.type) {
      case "log":
        appendLog([msg.line]);
        if (/Backup saved|downloaded \(|updated to build|added to downloaded servers|restored\.$|New world prepared/.test(msg.line)) loadState(false);
        // OP/whitelist changed via console or in-game: re-read once the server has written the file
        if (/Made \w+ (a server operator|no longer a server operator)|(Added|Removed) \w+ (to|from) the whitelist|Whitelist is now turned (on|off)/i.test(msg.line)) {
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
    $("#ws-indicator").classList.remove("on");
    if (ws !== ui.ws) return; // superseded or logged out
    if (ev.code === 4401) return showLogin();
    scheduleReconnect();
  };
}

function scheduleReconnect() {
  setTimeout(async () => {
    if ($("#app").classList.contains("hidden")) return; // logged out meanwhile
    try {
      await loadState(true); // resync everything missed while disconnected
      connect();
    } catch {
      scheduleReconnect(); // panel unreachable (e.g. container restarting): keep trying
    }
  }, ui.wsRetry);
  ui.wsRetry = Math.min(ui.wsRetry * 2, 15000);
}

async function loadState(full) {
  const data = await api("/api/state");
  const firstLoad = !ui.sel;
  ui.settings = data.settings;
  ui.world = data.world;
  ui.installed = data.installed;
  ui.java = Object.keys(data.java).map(Number);
  if (firstLoad) {
    ui.sel = { flavor: data.settings.flavor, version: data.settings.version || null };
    document.querySelector(`input[name="flavor"][value="${ui.sel.flavor}"]`).checked = true;
  }
  renderStatus(data.status);
  renderBackups(data.backups, data.backup_keep, data.backup_interval);
  const javas = Object.keys(data.java);
  $("#java-list").textContent = javas.length ? `Java ${javas.join(" · ")}` : "no Java found";
  if (full) {
    $("#console").replaceChildren();
    appendLog(data.log);
    $("#chat").replaceChildren();
    if (!data.chat.length) $("#chat").append(el("div", { className: "empty", textContent: "No chat messages yet" }));
    data.chat.forEach(appendChat);
    $("#min-ram").value = data.settings.min_ram;
    $("#max-ram").value = data.settings.max_ram;
    $("#map-enabled").checked = data.settings.map_enabled;
  }
  if (full) loadAccess();
  loadDatapacks();
  if (firstLoad || full) await loadVersions();
  else { renderToggle(); renderHint(); }
}

async function boot() {
  await loadState(true);
  $("#login").classList.add("hidden");
  $("#app").classList.remove("hidden");
  connect();
}

boot().catch((err) => {
  // 401 already shows the login screen; anything else (panel unreachable) too, with the error
  if ($("#login").classList.contains("hidden")) {
    showLogin();
    $("#login-error").textContent = err.message;
  }
});
