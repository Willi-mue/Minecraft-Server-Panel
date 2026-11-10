"use strict";

// Hall of fame on the live map: tier list (by share of advancements done), records ("who is the
// best at X": movement, combat, mining, life), monster hunters per mob, every advancement with who
// has it, and a page per player. Data: stats.json of the server (app/leaderboard.py); while players
// are online the server is asked to save first (`saving`), then the page reloads the numbers once.
// "Me": the player this browser picked ("This is me", kept in localStorage per server), else the
// server's guess from the address of the last login (me.json), else a picker.
// Uses skinUrl() from map.js for heads.

(() => {
  const TIER_INFO = { S: "Legend", A: "Veteran", B: "Adventurer", C: "Explorer", D: "Newcomer" };
  const CATEGORY = { story: "Minecraft", nether: "Nether", end: "The End", adventure: "Adventure", husbandry: "Husbandry" };
  const FRAME = { task: "Advancement", goal: "Goal", challenge: "Challenge" };
  const GROUPS = { Movement: "On the move", Combat: "Combat", Mining: "Mining & lumberjacking", Life: "Life & fun" };
  const MEDALS = ["gold", "silver", "bronze"];

  const h = (tag, props = {}, ...kids) => {
    const e = Object.assign(document.createElement(tag), props);
    e.append(...kids.filter((k) => k != null && k !== false));
    return e;
  };
  const num = (v, digits = 1) => Number(v).toLocaleString("en", { maximumFractionDigits: digits });
  /** A value with its unit: metres become km from 1000 m. */
  function fmt(v, unit = "") {
    if (unit === "m") return v >= 1000 ? `${num(v / 1000)} km` : `${num(v, 0)} m`;
    if (unit === "♥") return `${num(v, 0)} ♥`;
    return unit ? `${num(v)} ${unit}` : num(v, 0);
  }
  const pct = (share) => `${Math.round(share * 100)} %`;

  function head(p, cls = "lb-head") {
    const el = h("span", { className: cls });
    el.style.setProperty("--hue", String([...p.name].reduce((a, c) => (a * 31 + c.charCodeAt(0)) % 360, 7)));
    el.textContent = p.name[0].toUpperCase();
    if (p.skin && typeof skinUrl === "function") {
      skinUrl(p.skin, "head").then((src) => {
        if (!src) return;
        el.textContent = "";
        el.append(h("img", { src, alt: "" }));
        el.classList.add("has-skin");
      });
    }
    return el;
  }

  // ---------- modal ----------

  const overlay = h("div", { className: "lb-overlay hidden", role: "dialog", ariaModal: "true", ariaLabel: "Hall of fame" });
  const box = h("div", { className: "lb-box" });
  const sub = h("div", { className: "lb-sub" });
  const close = h("button", { className: "lb-close", type: "button", ariaLabel: "Close", title: "Close (Esc)", textContent: "×" });
  const tabs = h("div", { className: "lb-tabs", role: "tablist" });
  const body = h("div", { className: "lb-body" });
  box.append(h("div", { className: "lb-top" }, h("div", {}, h("h2", { textContent: "Hall of Fame" }), sub), close), tabs, body);
  overlay.append(box);
  document.body.append(overlay);

  let data = null, view = null, player = null, reload = 0, guess = null;
  const VIEWS = [["me", "Me"], ["tiers", "Tier list"], ["records", "Records"], ["hunters", "Monster hunters"], ["advancements", "Advancements"]];

  // who "Me" is: picked in this browser (per server: the map of another server has another path)
  const ME_KEY = `lb-me:${location.pathname}`;
  const picked = () => { try { return localStorage.getItem(ME_KEY); } catch { return null; } };
  const pick = (name) => {
    try {
      if (name) localStorage.setItem(ME_KEY, name); else localStorage.removeItem(ME_KEY);
    } catch { /* not kept: this visit only */ }
  };
  const meName = () => picked() || guess;
  const mePlayer = () => data?.players.find((p) => p.name === meName()) || null;
  for (const [id, label] of VIEWS) {
    const b = h("button", { type: "button", className: "lb-tab", role: "tab", textContent: label });
    b.dataset.view = id;
    b.addEventListener("click", () => { player = null; view = id; render(); });
    tabs.append(b);
  }

  function load(first) {
    clearTimeout(reload);
    return fetch("stats.json", { cache: "no-cache" })
      .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
      .then((d) => {
        const keepScroll = body.scrollTop;
        data = d;
        if (player) player = data.players.find((p) => p.name === player.name) || null;
        render(!first);
        if (!first) body.scrollTop = keepScroll;
        // the server was asked to save: its files are written in a moment, then load once more
        if (d.saving && !overlay.classList.contains("hidden")) reload = setTimeout(() => load(false), 5000);
      });
  }

  function open() {
    overlay.classList.remove("hidden");
    body.replaceChildren(h("div", { className: "lb-empty", textContent: "Loading…" }));
    const first = view === null;
    if (first) view = picked() ? "me" : "tiers"; // first time: start with yourself
    fetch("me.json", { cache: "no-store" }).then((r) => (r.ok ? r.json() : {})).catch(() => ({}))
      .then((m) => {
        guess = m.name || null;
        if (first && guess && !picked() && view === "tiers") view = "me"; // recognised by the connection
        if (data && !player && view === "me") render(true);
      });
    load(true).catch(() => body.replaceChildren(h("div", { className: "lb-empty", textContent: "The statistics could not be loaded." })));
  }
  const hide = () => { overlay.classList.add("hidden"); clearTimeout(reload); };
  close.addEventListener("click", hide);
  overlay.addEventListener("click", (e) => { if (e.target === overlay) hide(); });
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !overlay.classList.contains("hidden")) {
      e.stopPropagation();
      if (player) { player = null; render(); } else hide();
    }
  }, true);

  function render(keep = false) {
    for (const t of tabs.children) t.setAttribute("aria-selected", String(!player && t.dataset.view === view));
    const n = data.players.length;
    sub.textContent = n ? `${n} player${n === 1 ? "" : "s"} · ${data.total} advancements to get${data.saving ? " · updating…" : ""}` : "";
    if (!n) {
      body.replaceChildren(h("div", { className: "lb-empty", textContent: "Nobody has played on this server yet." }));
      return;
    }
    const pages = { me: meView, records, hunters, advancements, tiers };
    body.replaceChildren(player ? playerPage(player) : (pages[view] || tiers)());
    if (!keep) body.scrollTop = 0;
  }

  function showPlayer(p) {
    player = p;
    render();
  }

  function chip(p, extra, cls = "lb-chip") {
    const b = h("button", { type: "button", className: cls, title: p.name }, head(p), h("span", { className: "lb-chip-name", textContent: p.name }), extra);
    b.addEventListener("click", () => showPlayer(p));
    return b;
  }

  /** Top 3 as rows with gold / silver / bronze. */
  function ranking(list, value) {
    return h("ol", { className: "lb-rank" }, ...list.slice(0, 3).map((p, i) => {
      const row = h("li", { className: `medal-${MEDALS[i]}` },
        h("span", { className: "lb-place", textContent: String(i + 1) }),
        chip(p, null, "lb-chip flat"),
        h("span", { className: "lb-rank-val", textContent: value(p) }));
      return row;
    }));
  }

  // ---------- me ----------

  function meView() {
    const p = mePlayer();
    if (p) return playerPage(p, true);
    const grid = h("div", { className: "lb-tier-row lb-pick" }, ...data.players.map((q) => {
      const b = h("button", { type: "button", className: "lb-chip", title: `I am ${q.name}` }, head(q), h("span", { className: "lb-chip-name", textContent: q.name }));
      b.addEventListener("click", () => { pick(q.name); render(); });
      return b;
    }));
    return h("div", { className: "lb-me-pick" },
      h("h3", { className: "lb-cat", textContent: "Who are you?" }),
      h("p", { className: "lb-note", textContent: "Pick yourself once: this browser remembers it, and \"Me\" always shows your page." }),
      grid);
  }

  // ---------- tier list ----------

  function tiers() {
    const wrap = h("div", { className: "lb-tiers" });
    data.tiers.forEach(({ tier, from }, i) => {
      const upper = i ? data.tiers[i - 1].from : 1;
      const who = data.players.filter((p) => p.tier === tier);
      wrap.append(h("div", { className: `lb-tier tier-${tier}` },
        h("div", { className: "lb-tier-label" }, h("b", { textContent: tier }), h("small", { textContent: TIER_INFO[tier] || "" })),
        h("div", { className: "lb-tier-row" },
          ...(who.length ? who.map((p) => chip(p, h("span", { className: "lb-chip-val", textContent: pct(p.share) })))
            : [h("span", { className: "lb-tier-free", textContent: i ? `${Math.round(from * 100)}–${Math.round(upper * 100)} % of all advancements` : `${Math.round(from * 100)} %+ of all advancements` })]))));
    });
    return h("div", {}, wrap, h("p", { className: "lb-note", textContent: "Tiers by the share of all advancements done (recipes don't count). Click a player for details." }));
  }

  // ---------- records ----------

  function records() {
    const wrap = h("div", {});
    for (const [group, label] of Object.entries(GROUPS)) {
      const grid = h("div", { className: "lb-records" });
      for (const r of data.records.filter((x) => x.group === group)) {
        const ranked = data.players.filter((p) => p.stats[r.key] > 0).sort((a, b) => b.stats[r.key] - a.stats[r.key]);
        if (!ranked.length) continue;
        grid.append(h("div", { className: "lb-record" },
          h("div", { className: "lb-record-title", textContent: r.title }),
          ranking(ranked, (p) => fmt(p.stats[r.key], r.unit))));
      }
      if (grid.childElementCount) wrap.append(h("h3", { className: "lb-cat", textContent: label }), grid);
    }
    if (!wrap.childElementCount) wrap.append(h("div", { className: "lb-empty", textContent: "No records yet: go and play!" }));
    else wrap.append(h("p", { className: "lb-note", textContent: "Records nobody has yet are hidden. Minecraft saves the numbers every few minutes." }));
    return wrap;
  }

  // ---------- monster hunters ----------

  function hunters() {
    const mobs = Object.keys(data.mobs);
    const total = (key, mob) => data.players.reduce((s, p) => s + (p[key][mob] || 0), 0);
    const section = (key, title, note, verb) => {
      const list = mobs.filter((m) => total(key, m) > 0).sort((a, b) => total(key, b) - total(key, a));
      if (!list.length) return null;
      const grid = h("div", { className: "lb-records" }, ...list.map((mob) => {
        const ranked = data.players.filter((p) => p[key][mob] > 0).sort((a, b) => b[key][mob] - a[key][mob]);
        return h("div", { className: "lb-record" },
          h("div", { className: "lb-record-title" }, data.mobs[mob], h("small", { textContent: ` ${num(total(key, mob), 0)} ${verb}` })),
          ranking(ranked, (p) => `${num(p[key][mob], 0)}×`));
      }));
      return h("div", { className: "lb-hunt" }, h("h3", { className: "lb-cat" }, title, h("small", { textContent: ` ${note}` })), grid);
    };
    const killed = section("killed", "Monster hunters", "who killed the most of each kind", "killed");
    const nemesis = section("killed_by", "Nemesis", "who was killed the most by each kind", "kills");
    return killed || nemesis ? h("div", {}, killed, nemesis) : h("div", { className: "lb-empty", textContent: "Nobody has killed anything yet." });
  }

  // ---------- advancements ----------

  function advItem(a, who, locked) {
    return h("li", { className: `lb-adv frame-${a.frame} ${locked ? "locked" : ""}`, title: a.description },
      h("span", { className: "lb-adv-icon", ariaHidden: "true" }),
      h("div", { className: "lb-adv-text" },
        h("div", { className: "lb-adv-title" }, a.title, a.frame !== "task" ? h("span", { className: "lb-badge", textContent: FRAME[a.frame] }) : null),
        h("div", { className: "lb-adv-desc", textContent: a.description })),
      who);
  }

  function advancements() {
    const owners = new Map();
    for (const p of data.players) for (const a of p.done) (owners.get(a) || owners.set(a, []).get(a)).push(p);
    const groups = new Map();
    for (const [id, a] of Object.entries(data.advancements)) {
      if (!groups.has(a.category)) groups.set(a.category, []);
      groups.get(a.category).push([id, a]);
    }
    const n = data.players.length;
    const order = Object.keys(CATEGORY);
    const wrap = h("div", { className: "lb-advs" });
    for (const [cat, list] of [...groups].sort((a, b) => order.indexOf(a[0]) - order.indexOf(b[0]))) {
      list.sort((a, b) => (owners.get(b[0])?.length || 0) - (owners.get(a[0])?.length || 0) || a[1].title.localeCompare(b[1].title));
      const got = list.filter(([id]) => owners.has(id)).length;
      wrap.append(h("h3", { className: "lb-cat" }, CATEGORY[cat] || cat, h("small", { textContent: ` ${got} / ${list.length} reached by someone` })));
      wrap.append(h("ul", { className: "lb-adv-list" }, ...list.map(([id, a]) => {
        const who = owners.get(id) || [];
        const rarity = who.length ? (who.length / n <= 0.1 ? "legendary" : who.length / n <= 0.34 ? "rare" : "common") : "none";
        return advItem(a, h("div", { className: "lb-adv-who" },
          ...who.slice(0, 6).map((p) => {
            const b = h("button", { type: "button", className: "lb-mini", title: p.name }, head(p, "lb-head small"));
            b.addEventListener("click", () => showPlayer(p));
            return b;
          }),
          who.length > 6 ? h("span", { className: "lb-more", textContent: `+${who.length - 6}` }) : null,
          h("span", { className: `lb-rarity ${rarity}`, textContent: who.length ? `${Math.round((who.length / n) * 100)} %` : "nobody" })), !who.length);
      })));
    }
    return wrap;
  }

  // ---------- one player ----------

  function playerPage(p, asMe = false) {
    const back = h("button", { type: "button", className: "lb-back", textContent: "← Back" });
    back.addEventListener("click", () => { player = null; render(); });
    // "This is me" / "Not me": what the Me tab shows
    const isMe = meName() === p.name;
    const meBtn = h("button", { type: "button", className: `lb-me-btn${isMe ? " on" : ""}`,
      textContent: isMe ? "That's me ✓" : "This is me",
      title: isMe ? (picked() ? "Click if this is not you" : "Recognised by your connection. Click if this is not you") : "Show this player under Me" });
    meBtn.addEventListener("click", () => {
      if (isMe) {
        pick(null);
        if (guess === p.name) guess = null; // the guess was wrong
      } else pick(p.name);
      render(true);
    });
    const rank = data.players.indexOf(p) + 1;
    const figure = h("div", { className: "lb-figure" });
    if (p.skin && typeof skinUrl === "function") {
      skinUrl(p.skin, "body").then((src) => src && figure.append(h("img", { src, alt: "" })));
    } else figure.append(head(p, "lb-head huge"));
    // where this player stands in each record (1st place gets a medal)
    const place = (key) => {
      const better = data.players.filter((o) => o.stats[key] > p.stats[key]).length;
      return p.stats[key] > 0 && better < 3 ? MEDALS[better] : null;
    };
    const statGroups = Object.entries(GROUPS).map(([group, label]) => h("div", { className: "lb-stat-group" },
      h("div", { className: "lb-stat-title", textContent: label }),
      h("div", { className: "lb-stats" }, ...data.records.filter((r) => r.group === group).map((r) => {
        const medal = place(r.key);
        return h("div", { className: medal ? `medal-${medal}` : "", title: medal ? `#${MEDALS.indexOf(medal) + 1} on this server` : "" },
          h("span", { textContent: r.title }), h("b", { textContent: fmt(p.stats[r.key] ?? 0, r.unit) }));
      }))));
    const top = (obj) => Object.entries(obj).sort((a, b) => b[1] - a[1]).slice(0, 5);
    const mobList = (obj, empty) => {
      const t = top(obj);
      return t.length ? h("ol", { className: "lb-mobs" }, ...t.map(([m, c]) => h("li", {}, h("span", { textContent: data.mobs[m] || m }), h("b", { textContent: `${num(c, 0)}×` }))))
        : h("p", { className: "lb-note", textContent: empty });
    };
    const done = new Set(p.done);
    const all = Object.entries(data.advancements);
    const got = all.filter(([id]) => done.has(id)), missing = all.filter(([id]) => !done.has(id));
    const advList = (items, locked) => h("ul", { className: "lb-adv-list compact" }, ...items.map(([, a]) => advItem(a, null, locked)));
    return h("div", { className: "lb-player" },
      asMe ? null : back,
      h("div", { className: "lb-player-top" }, figure,
        h("div", { className: "lb-player-info" },
          h("div", { className: "lb-player-name" }, p.name, h("span", { className: `lb-tier-tag tier-${p.tier}`, textContent: `${p.tier} · ${TIER_INFO[p.tier]}` }), meBtn),
          h("div", { className: "lb-sub", textContent: `#${rank} of ${data.players.length} · ${p.done.length} of ${data.total} advancements` }),
          h("div", { className: "lb-progress" }, h("span", { style: `width: ${Math.max(2, p.share * 100)}%` })),
          h("div", { className: "lb-prey" },
            h("div", {}, h("div", { className: "lb-stat-title", textContent: "Favourite prey" }), mobList(p.killed, "Hasn't killed anything yet.")),
            h("div", {}, h("div", { className: "lb-stat-title", textContent: "Nemesis" }), mobList(p.killed_by, "Never killed by a mob."))))),
      ...statGroups,
      h("h3", { className: "lb-cat" }, "Advancements done", h("small", { textContent: ` ${got.length}` })),
      got.length ? advList(got, false) : h("p", { className: "lb-note", textContent: "Nothing yet." }),
      h("h3", { className: "lb-cat" }, "Still to get", h("small", { textContent: ` ${missing.length}` })),
      advList(missing, true));
  }

  // ---------- button in the top bar ----------

  const btn = h("button", { type: "button", id: "lb-btn", className: "mode icon", title: "Hall of fame: tier list and records", ariaLabel: "Hall of fame" });
  btn.innerHTML = '<svg viewBox="0 0 24 24" width="15" height="15" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 21h8M12 17v4M7 4h10v5a5 5 0 0 1-10 0z"/><path d="M17 5h3v2a3 3 0 0 1-3 3M7 5H4v2a3 3 0 0 0 3 3"/></svg>';
  btn.addEventListener("click", open);
  document.querySelector(".panel.top-left")?.append(btn);
})();
