# TODO – Dashboard (Panel)

Code-Aufgaben für das Panel. Entscheidungen/Infos vom Entwickler: [DEV_TODO_DASHBOARD.md](DEV_TODO_DASHBOARD.md).
Die Live-Map hat eigene Listen: [TODO_LIVEMAP.md](TODO_LIVEMAP.md), [DEV_TODO_LIVEMAP.md](DEV_TODO_LIVEMAP.md).

**Zuständigkeit** (damit zwei Claudes parallel arbeiten können): `panel/` außer `panel/src/livemap.rs`,
`mapserver.rs`, `leaderboard.rs`, `waypoints.rs`; `app/static/` außer `app/static/map/`; außerdem
`docker/` und `datapacks/`. `mapgen/` nicht anfassen. Gemeinsam genutzte Dateien (`panel/src/main.rs`, `api.rs`,
`server.rs`, `config.rs`, `tasks.rs`, `Dockerfile`, `docker-compose*.yml`, `README.md`, `CLAUDE.md`) nur
gezielt und klein ändern und direkt vorher neu lesen – dort arbeitet evtl. gleichzeitig die Live-Map-Seite.

## Offen

- [ ] Feuerwerk bei Advancements
  - Bei jedem Advancement ein Feuerwerk über dem Spieler, bei Challenges ein großes (mehrere Raketen, Farben).
  - Datapacks haben keinen Trigger „irgendein Advancement“ (nur pro Advancement über `rewards.function`, dafür
    müsste man alle Vanilla-Advancements überschreiben → bricht bei Updates). Vorschlag: im Panel —
    `server.rs` erkennt schon Konsolenzeilen; neue Regex für „has made the advancement“ / „has reached the goal“ /
    „has completed the challenge“, dann `execute at <name> run summon firework_rocket …` (Name validiert,
    Befehl serverseitig gebaut). Braucht die Spielregel `announceAdvancements` (Default an); Schalter in den
    Panel-Einstellungen pro Server.
  - Eigene Server-Advancements können es zusätzlich direkt per `rewards.function` auslösen.
- [ ] Mods-/Plugins-Seite: Ladeanzeige während einer Aktion wie bei den Datapacks (`packBusy` in app.js) —
  bisher passiert bis zur Antwort sichtbar nichts.

## Eigene Datapacks

Quellen in `datapacks/` (Aufbau, Konventionen, Tests: [datapacks/README.md](datapacks/README.md)); Ideen und
Entscheidungen: [DEV_TODO_DASHBOARD.md](DEV_TODO_DASHBOARD.md) → „Datapack-Ideen“. Als Nächstes: das Event-Pack.

- [ ] Optional: Grab-Wegpunkt auf der Live-Map („Grab von <Name>“). Datapacks können keine Dateien schreiben →
  das Panel müsste eine Chat-/Log-Zeile erkennen (ähnlich `LOGIN_RE`) und `waypoints.rs` nutzen (Live-Map-Seite).
- [ ] Optional: zweite Bibliotheksquelle im Panel (`DATAPACK_SRC`, read-only gemountet), falls das Kopieren von
  `datapacks/` nach `data/datapacks/` nervt.
