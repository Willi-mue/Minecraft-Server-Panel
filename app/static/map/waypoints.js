"use strict";

// Waypoints: named spots per server (app/waypoints.py, waypoints.json). Everyone sees all of them.
// A new one belongs to "you": the player picked in this browser (shared with the hall of fame's
// "Me") or the server's guess from the address of your last login. Changing / deleting: the
// browser that created it (a random key kept in localStorage), the owner or an operator recognised
// by their login address, and the panel; the server tells which ones (`can_edit`).
// Uses map.js globals: state, map, toLatLng, markerIcon, goTo, showToast, copyCommand, tpCommand,
// update3dMarkers, NAMES, settingsPanel, settingsButton.

const WP_ME_KEY = `lb-me:${location.pathname}`; // = ME_KEY in leaderboard.js
const WP_KEY = "map-key";
const WP_COLORS = ["green", "blue", "red", "yellow", "purple", "orange", "white"];

const wp = {
  list: [], me: null, players: [], colors: WP_COLORS, limits: { name: 32 },
  layer: L.layerGroup().addTo(map),
  editing: null, // null | "new" | waypoint id
  highlight: null,
  key: null,
};

const wpEl = (tag, props = {}, ...kids) => {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...kids.filter((k) => k != null && k !== false));
  return e;
};

function wpRead(key) {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function wpStore(key, value) {
  try {
    if (value == null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch { /* this visit only */ }
}

/** This browser's secret: the server keeps only its hash with the waypoints made here. */
function wpBrowserKey() {
  if (wp.key) return wp.key;
  let k = wpRead(WP_KEY);
  if (!k || !/^[A-Za-z0-9_-]{16,128}$/.test(k)) {
    const bytes = crypto.getRandomValues(new Uint8Array(24));
    k = btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    wpStore(WP_KEY, k);
  }
  return (wp.key = k);
}

async function wpRequest(method, path, body) {
  const headers = { "X-Map-Key": wpBrowserKey() };
  const me = wpRead(WP_ME_KEY);
  if (me && /^[A-Za-z0-9_]{1,16}$/.test(me)) headers["X-Map-Player"] = me;
  if (body) headers["Content-Type"] = "application/json";
  const res = await fetch(path, { method, headers, body: body ? JSON.stringify(body) : undefined, cache: "no-store" });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.detail || `Error ${res.status}`);
  return data;
}

async function loadWaypoints() {
  try {
    const d = await wpRequest("GET", "waypoints.json");
    wp.list = d.waypoints || [];
    wp.me = d.me || null;
    wp.players = d.players || [];
    wp.colors = d.colors || WP_COLORS;
    wp.limits = d.limits || wp.limits;
    drawWaypoints();
  } catch { /* map off / offline: keep the last state */ }
}

const wpSpot = (w) => ({ dimension: w.dim, x: w.x + 0.5, y: w.y ?? undefined, z: w.z + 0.5 });

/** Markers of the current dimension (2D), the 3D markers and the open list. Called by showDim. */
function drawWaypoints() {
  state.waypoints = wp.list;
  wp.layer.clearLayers();
  for (const w of wp.list) {
    if (w.dim !== state.dim) continue;
    L.marker(toLatLng(w.x + 0.5, w.z + 0.5), {
      icon: markerIcon(w.name, "waypoint", null, w), interactive: false, keyboard: false, zIndexOffset: -60,
    }).addTo(wp.layer);
  }
  update3dMarkers();
  renderWpMe();
  renderWpList();
}

// ---------- panel ----------

const wpPanel = wpEl("div", { id: "waypoints", className: "panel wp-panel hidden" });
const wpMeSelect = wpEl("select", { className: "server-select", ariaLabel: "Which player you are" });
const wpMeNote = wpEl("div", { className: "hint" });
const wpForm = wpEl("form", { className: "wp-form hidden" });
const wpAdd = wpEl("button", { type: "button", className: "mode small", textContent: "+ Add the picked spot" });
const wpList = wpEl("div", { className: "wp-list" });
const wpClose = wpEl("button", { type: "button", className: "clear", textContent: "×", title: "Close", ariaLabel: "Close" });
wpPanel.append(
  wpEl("div", { className: "wp-head" }, wpEl("b", { textContent: "Waypoints" }), wpClose),
  wpEl("label", { className: "wp-me" }, wpEl("span", { textContent: "You are" }), wpMeSelect),
  wpMeNote, wpAdd, wpForm, wpList);
document.body.append(wpPanel);

const wpButton = wpEl("button", { type: "button", id: "wp-btn", className: "mode icon", title: "Waypoints", ariaLabel: "Waypoints" });
wpButton.innerHTML = '<svg viewBox="0 0 24 24" width="15" height="15" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 21V4"/><path d="M5 4h12l-2.5 4L17 12H5"/></svg>';
document.querySelector(".panel.top-left")?.append(wpButton);

function openWaypoints(show = true) {
  wpPanel.classList.toggle("hidden", !show);
  wpButton.classList.toggle("active", show);
  if (show) {
    settingsPanel.classList.add("hidden"); // same place on the screen
    settingsButton.classList.remove("active");
    loadWaypoints();
  } else closeWpForm();
}

function closeWaypoints() {
  if (!wpPanel.classList.contains("hidden")) openWaypoints(false);
}

wpButton.addEventListener("click", () => openWaypoints(wpPanel.classList.contains("hidden")));
wpClose.addEventListener("click", () => openWaypoints(false));
settingsButton.addEventListener("click", closeWaypoints);

function renderWpMe() {
  const current = wp.me?.name || "";
  const names = [...wp.players];
  if (current && !names.includes(current)) names.unshift(current);
  wpMeSelect.replaceChildren(new Option("– pick yourself –", ""), ...names.map((n) => new Option(n, n, false, n === current)));
  wpMeSelect.value = current;
  const picked = wpRead(WP_ME_KEY);
  wpMeNote.textContent = wp.me?.op ? "Recognised as an operator: you can change every waypoint."
    : current && !picked ? "Recognised by your connection." : "";
  wpMeNote.classList.toggle("hidden", !wpMeNote.textContent);
}

wpMeSelect.addEventListener("change", () => {
  wpStore(WP_ME_KEY, wpMeSelect.value || null);
  loadWaypoints();
});

function dimLabel(slug) {
  return NAMES[slug] || slug;
}

function renderWpList() {
  if (wpPanel.classList.contains("hidden")) return;
  wpAdd.disabled = !state.picked;
  wpAdd.title = state.picked ? "Save the spot you clicked as a waypoint" : "Click a spot on the map first";
  if (!wp.list.length) {
    wpList.replaceChildren(wpEl("div", { className: "muted", textContent: "No waypoints yet. Click a spot on the map, then add it here." }));
    return;
  }
  // this dimension first, then by name
  const sorted = [...wp.list].sort((a, b) => (a.dim !== state.dim) - (b.dim !== state.dim) || a.name.localeCompare(b.name));
  wpList.replaceChildren(...sorted.map(wpRow));
}

function wpRow(w) {
  const row = wpEl("div", { className: `wp-row${wp.highlight === w.id ? " active" : ""}`, title: "Show on the map" });
  const where = `${w.x} ${w.y ?? "~"} ${w.z}` + (w.dim !== state.dim ? ` · ${dimLabel(w.dim)}` : "");
  const text = wpEl("div", { className: "wp-text" },
    wpEl("div", { className: "wp-name", textContent: w.name }),
    wpEl("div", { className: "wp-where", textContent: w.owner ? `${where} · ${w.owner}` : where }));
  const tools = wpEl("div", { className: "wp-tools" });
  const tool = (label, title, fn) => {
    const b = wpEl("button", { type: "button", className: "wp-tool", textContent: label, title, ariaLabel: title });
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      fn();
    });
    tools.append(b);
  };
  tool("/tp", "Copy a teleport command for this waypoint", () => copyCommand(tpCommand({ dim: w.dim, x: w.x, y: w.y, z: w.z })));
  if (w.can_edit) {
    tool("✎", "Rename / change colour", () => openWpForm(w));
    tool("×", "Delete", () => deleteWaypoint(w));
  }
  row.append(wpEl("span", { className: `wp-dot wp-${w.color}` }), text, tools);
  row.addEventListener("click", () => {
    wp.highlight = w.id;
    goTo(wpSpot(w));
    renderWpList();
  });
  return row;
}

async function deleteWaypoint(w) {
  if (!confirm(`Delete the waypoint "${w.name}"?`)) return;
  try {
    await wpRequest("DELETE", `waypoints/${w.id}`);
    showToast(`Deleted "${w.name}"`);
  } catch (e) {
    showToast(e.message, 3000);
  }
  loadWaypoints();
}

// ---------- add / edit form ----------

function closeWpForm() {
  wp.editing = null;
  wpForm.classList.add("hidden");
  wpForm.replaceChildren();
  wpAdd.classList.remove("hidden");
}

function openWpForm(w = null) {
  if (!w && !state.picked) {
    showToast("Click a spot on the map first", 2500);
    return;
  }
  if (!w && !wp.me?.name) {
    showToast("Pick which player you are first", 2500);
    wpMeSelect.focus();
    return;
  }
  wp.editing = w ? w.id : "new";
  let color = w ? w.color : wp.colors[0];
  const name = wpEl("input", { type: "text", required: true, maxLength: wp.limits.name, value: w ? w.name : "", placeholder: "Name, e.g. Home" });
  const swatches = wpEl("div", { className: "wp-colors" }, ...wp.colors.map((c) => {
    const b = wpEl("button", { type: "button", className: `wp-swatch wp-${c}${c === color ? " on" : ""}`, title: c, ariaLabel: c });
    b.addEventListener("click", () => {
      color = c;
      for (const s of swatches.children) s.classList.toggle("on", s === b);
    });
    return b;
  }));
  const pk = state.picked;
  const spot = (p) => `${p.x} ${p.y ?? "~"} ${p.z}${p.dim !== "overworld" ? ` (${dimLabel(p.dim)})` : ""}`;
  let move = null;
  const pos = wpEl("div", { className: "wp-where" });
  if (w) {
    pos.textContent = `At ${spot(w)}`;
    if (pk) move = wpEl("label", { className: "wp-move" }, wpEl("input", { type: "checkbox" }), ` Move to the picked spot ${spot(pk)}`);
  } else pos.textContent = `At ${spot(pk)}`;
  const save = wpEl("button", { type: "submit", className: "mode small", textContent: w ? "Save" : "Add waypoint" });
  const cancel = wpEl("button", { type: "button", className: "mode small", textContent: "Cancel" });
  cancel.addEventListener("click", closeWpForm);
  wpForm.replaceChildren(name, swatches, pos, move, wpEl("div", { className: "wp-buttons" }, save, cancel));
  wpForm.onsubmit = async (e) => {
    e.preventDefault();
    const body = { name: name.value, color };
    const target = w ? (move?.querySelector("input").checked ? pk : null) : pk;
    if (target) Object.assign(body, { dim: target.dim, x: target.x, y: target.y, z: target.z });
    save.disabled = true;
    try {
      const saved = w ? await wpRequest("PUT", `waypoints/${w.id}`, body) : await wpRequest("POST", "waypoints", body);
      wp.highlight = saved.id;
      showToast(w ? `Saved "${saved.name}"` : `Added "${saved.name}"`);
      if (target) clearPick(); // the waypoint marks the spot now
      closeWpForm();
      await loadWaypoints();
    } catch (err) {
      showToast(err.message, 3000);
      save.disabled = false;
    }
  };
  wpForm.classList.remove("hidden");
  wpAdd.classList.add("hidden");
  name.focus();
  name.select();
}

wpAdd.addEventListener("click", () => openWpForm());

// "+ Waypoint" in the picked-spot box (bottom left)
const wpPickedBtn = wpEl("button", { type: "button", className: "wp-picked", textContent: "+ Waypoint", title: "Save this spot as a waypoint" });
document.getElementById("picked-clear")?.before(wpPickedBtn);
wpPickedBtn.addEventListener("click", (e) => {
  e.stopPropagation(); // not the box's "copy /tp"
  openWaypoints(true);
  openWpForm();
});

// A click on a waypoint marker (2D or 3D) opens its entry in the list, without picking the spot
// or starting a drag underneath.
for (const type of ["pointerdown", "mousedown", "touchstart", "dblclick"]) {
  document.addEventListener(type, (e) => {
    if (e.target instanceof Element && e.target.closest(".marker.waypoint")) e.stopPropagation();
  }, true);
}
document.addEventListener("click", (e) => {
  const marker = e.target instanceof Element && e.target.closest(".marker.waypoint");
  if (!marker) return;
  e.stopPropagation();
  wp.highlight = marker.dataset.wp;
  openWaypoints(true);
  renderWpList();
  wpList.querySelector(".wp-row.active")?.scrollIntoView({ block: "nearest" });
}, true);

loadWaypoints();
setInterval(loadWaypoints, 15000);
