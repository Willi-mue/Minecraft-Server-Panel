# TODO – Dashboard (Panel)

Code-Aufgaben für das Panel. Entscheidungen/Infos vom Entwickler: [DEV_TODO_DASHBOARD.md](DEV_TODO_DASHBOARD.md).
Die Live-Map hat eigene Listen: [TODO_LIVEMAP.md](TODO_LIVEMAP.md), [DEV_TODO_LIVEMAP.md](DEV_TODO_LIVEMAP.md).

**Zuständigkeit** (damit zwei Claudes parallel arbeiten können): `app/` außer `app/livemap.py`,
`app/mapserver.py` und `app/static/map/`; außerdem `docker/` und die Tests dieser Module.
`mapgen/` nicht anfassen. Gemeinsam genutzte Dateien (`app/main.py`, `app/server.py`,
`app/config.py`, `Dockerfile`, `docker-compose*.yml`, `README.md`, `CLAUDE.md`) nur gezielt und klein
ändern und direkt vorher neu lesen – dort arbeitet evtl. gleichzeitig die Live-Map-Seite.

## Ideen für später (viel später – erst wenn Vanilla mit Datapacks rund läuft)

- [ ] Fabric als dritte Server-Variante
  - Approach: `FLAVORS` um `"fabric"` erweitern. Versionsliste aus der Fabric-Meta-API (`https://meta.fabricmc.net/v2/versions/game` für Spielversionen, `/v2/versions/loader/<game>` für den neuesten stabilen Loader, `/v2/versions/installer`). Server-Jar: `/v2/versions/loader/<game>/<loader>/<installer>/server/jar` (Launcher, lädt beim ersten Start Vanilla-Jar + Libraries nach `server/`). Fabric liefert keine Prüfsumme: nur Größe/ZIP-Gültigkeit prüfen. Sidecar-JSON: `java` aus Mojangs Version-JSON, `build` = Loader-Version. Frontend: dritte Option im Server-Typ-Auswahlfeld, Tag-Farbe, Texte so, dass Paper als Empfehlung für Vanilla-Spiele und Fabric für Mods erscheint. Paper-Update-Button analog für den Fabric-Loader.
  - Touches: `app/versions.py`, `app/main.py`, `app/static/app.js`, `app/static/index.html`, `tests/`

- [ ] Mod-Verwaltung für Fabric
  - Approach: Neues Modul `app/mods.py` für `server/mods/`: Liste (Dateiname, Größe, aus `fabric.mod.json` im Jar: id, Name, Version, `depends.minecraft`), Hochladen von `.jar` (Dateiname validieren, kein Pfad), Löschen, Aktivieren/Deaktivieren per Umbenennung in `.jar.disabled`. Änderungen nur bei gestopptem Server (sonst 409, wie bei Restore). Fabric API als Hinweis anbieten, weil fast alle Mods sie brauchen. Neue Karte "Mods" im Panel, nur sichtbar bei Flavor `fabric`. Später optional: Suche/Download über die Modrinth-API (`api.modrinth.com/v2`, gefiltert nach `loaders=fabric` und Spielversion, mit SHA-512-Prüfung).
  - Touches: `app/mods.py` (neu), `app/main.py`, `app/static/app.js`, `app/static/index.html`, `app/static/style.css`, `tests/test_mods.py`
