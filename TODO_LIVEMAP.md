# TODO – Live-Map

Code-Aufgaben für die Live-Map. Entscheidungen/Infos vom Entwickler: [DEV_TODO_LIVEMAP.md](DEV_TODO_LIVEMAP.md).
Das Panel (Dashboard) hat eigene Listen: [TODO_DASHBOARD.md](TODO_DASHBOARD.md), [DEV_TODO_DASHBOARD.md](DEV_TODO_DASHBOARD.md).

**Zuständigkeit** (damit zwei Claudes parallel arbeiten können): `mapgen/`, `app/livemap.py`,
`app/mapserver.py`, `app/static/map/` und die Tests dieser Module.
Gemeinsam genutzte Dateien (`app/main.py`, `app/server.py`, `app/config.py`, `Dockerfile`,
`docker-compose*.yml`, `README.md`, `CLAUDE.md`) nur gezielt und klein ändern und direkt vorher neu
lesen – dort arbeitet evtl. gleichzeitig die Dashboard-Seite.

## Eigene Live-Map in Rust (ersetzt squaremap, für jeden Server-Typ)

Entscheidungen vom 2026-11-09: eigener Renderer in Rust, Farben im Stil der Minecraft-Karten-Items
(eingebaute Tabelle, keine Mojang-Assets), erst 2D, dann leichtes 3D. Das Panel bleibt in Python.
Anpassungsmöglichkeiten der Karte besprechen wir später (siehe DEV_TODO_LIVEMAP).

### Stufe 1: 2D-Renderer `mapgen` (Rust) – erledigt 2026-11-09

- [x] Rust-Programm `mapgen/` (680–900 KB), Weltformate 26.x + alt + Paper-alt, 26.x-Palettenformen, LZ4/zlib/gzip/.mcc
- [x] Karten-Item-Farben (61 MapColors, Blockregeln), Schattierung inkl. Regionsgrenzen, Wassertiefe, Nether unter der Decke
- [x] Kacheln + Zoomstufen + 16-Bit-Höhenkacheln, inkrementell: Testwelt (8.270 Chunks) 1,9 s mit 2 Threads, ohne Änderungen 7 ms, eine geänderte Region 0,4 s
- [x] Kartenseite (Leaflet lokal), Dimensionen, Koordinaten, teilbarer Link, Live-Spieler ohne externe Dienste
- [x] Panel-Anbindung: niedrige Priorität, `MAP_THREADS`/`MAP_INTERVAL`, Render beim Start/Stop/"Render now", Spielerpositionen nur bei offener Karte, squaremap entfernt
- [x] Docker: mehrstufiger Build (Rust-Tests laufen im Build), Image +1 MB
- [x] Tests: 16 Rust-Tests, Python-Tests für Spielerpositionen/Aufräumen

### Stufe 1: Feinschliff

- [x] Schattierung an der Nordkante: nur die oberste Chunk-Reihe der südlichen Nachbarregion wird neu berechnet (32 statt 1024 Chunks), Ergebnis byte-genau wie ein kompletter Neu-Render
- [x] Spawn-Marker (26.x `Data.spawn` und altes `SpawnX/SpawnZ`)
- [x] Fehlende Kacheln als leeres Bild statt 404, Dimensionen in fester Reihenfolge
- [x] Weltgrenze anzeigen (26.x pro Dimension `data/minecraft/world_border.dat`, alt `level.dat` nur Overworld; nur wenn kleiner als Standard; gestrichelt, außen abgedunkelt)
- [x] Nether mit echter Welt geprüft (958 Chunks, 2026-11-09): Biome/Lava/Basalt plausibel; Löcher bei massiven Säulen ohne Höhle behoben (zeigt jetzt den Block unter der Bedrock-Decke)
- [ ] End mit größerer echter Welt prüfen (Testwelt hat dort weiterhin nur 1 fertigen Chunk)

- [x] Echte Block-Texturen statt Karten-Item-Farben (2026-11-09, Entscheidung Entwickler): Panel lädt den Minecraft-Client der Weltversion einmal von Mojang (SHA-1, `DATA_DIR/cache`, abschaltbar `MAP_TEXTURES=false`); mapgen baut daraus Blocktabelle + Textur-Atlas (Blockstates → Modelle, Pflanzen/Schienen/Fackeln durchsichtig, Biom-Färbung für Gras/Laub/Wasser) und schreibt `b/`-Kacheln (Block-ID + Biom je Säule). 2D: echte Farben, ab 4 px/Block Texturen; 3D: Texturen auf Ober- und Seitenflächen.
- [x] 3D mit echten Blöcken (2026-11-09): Blockstapel je Säule (`v/`-Kacheln) statt nur oberster Block – Klippen zeigen ihre Schichten, Baumkronen schweben mit Stamm; Pflanzen als gekreuzte Flächen, Ranken an Wänden, durchbrochenes Laub (Fancy), durchsichtiges Wasser mit Tiefe, Gras-Seiten-Overlay, Biomfarben weich überblendet; volle Blockdetails bis ~360 Blöcke Entfernung, keine Lücken an Detailgrenzen.
- [x] 3D-Korrekturen (2026-11-09): keine Löcher mehr unter Brücken/in Höhlen (Stapel endet immer auf festem Block, Höhlenwände), Kaktus/Stufen/Teppiche in echter Größe, keine bunten Linien an Blockkanten (eigene Mipmaps), weichere 2D-Schattierung ohne Baumschatten, mehrere Worker + Hysterese für flüssigeres Laden.
- [x] 3D-Korrekturen 2 (2026-11-09): Schienen in echter Richtung inkl. Kurven und Steigungen (Blockzustand `shape`); Höhlen zeigen echte Wandblöcke statt Ersatz-Steinwände (Nachbarsäulen exportieren ihre Blöcke bis zur tiefsten sichtbaren Luft, bis 8 Säulen in die Höhle hinein, auch über Regionsgrenzen; keine Ersatzwände/Löcher mehr nahe offener Stellen).
- [ ] Offen in 3D: Blockzustände (Treppen, Stufen, Zäune, Türen, Fackeln, Truhen) werden noch nicht als eigene Form gezeichnet; Schnee-/Teppichschichten erscheinen als ganze Blöcke; Pflanzen in 2D von oben nicht sichtbar.
- [x] Fehler behoben: 26.x speichert Blöcke mit Eigenschaften als `{id, properties}` – wurden als Luft gelesen (Blätter, Ranken, gedrehte Stämme fehlten auf der Karte).

### Stufe 2: leichtes 3D

- [x] 3D-Ansicht im Browser aus Farb- und Höhenkacheln (2026-11-09): **in Blöcken** wie bei anderen Minecraft-Karten (jede Säule ein Quader, Seiten bis zum tieferen Nachbarn, Flächenschattierung wie im Spiel, leichte Helligkeitsvariation je Block, Karten-Schattierung herausgerechnet). three.js r170 lokal, erst beim Umschalten geladen; Dekodieren und Netzbau im Web Worker; ein Netz pro 128er-Kachel, Blockgröße 1/2/4/8/16 je nach Entfernung; nur Regionen um die Kamera, Nebel; Spieler/Spawn als Marker, Koordinaten mit Höhe unter der Maus, Modus im teilbaren Link (`…/3d`). Server-Last unverändert.
  - Offen: Flüssigkeit auf einem echten Laptop prüfen (siehe DEV_TODO_LIVEMAP).

### Später (Optimierungen)

- [ ] Chunk-genaues inkrementelles Rendern
  - Approach: Zeitstempel pro Chunk aus dem Regions-Header merken und nur geänderte Chunks neu rendern (Farb- und Höhenkachel laden, betroffene Bereiche ersetzen).
- [ ] Optional Biom-Färbung (Gras/Laub/Wasser je Biom) als Kartenstil-Option.
