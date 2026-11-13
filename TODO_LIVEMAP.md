# TODO – Live-Map

Code-Aufgaben für die Live-Map. Entscheidungen/Infos vom Entwickler: [DEV_TODO_LIVEMAP.md](DEV_TODO_LIVEMAP.md).
Das Panel (Dashboard) hat eigene Listen: [TODO_DASHBOARD.md](TODO_DASHBOARD.md), [DEV_TODO_DASHBOARD.md](DEV_TODO_DASHBOARD.md).

**Zuständigkeit** (damit zwei Claudes parallel arbeiten können): `mapgen/`, `panel/src/livemap.rs`,
`mapserver.rs`, `leaderboard.rs`, `waypoints.rs`, `app/static/map/` und die Tests dieser Module.
Gemeinsam genutzte Dateien (`panel/src/main.rs`, `api.rs`, `server.rs`, `config.rs`, `tasks.rs`,
`skins.rs`, `Dockerfile`, `docker-compose*.yml`, `README.md`, `CLAUDE.md`) nur gezielt und klein ändern
und direkt vorher neu lesen – dort arbeitet evtl. gleichzeitig die Dashboard-Seite.
Seit 2026-11-11 ist das Backend Rust (`panel/`), das Python-Backend (`app/*.py`) ist gelöscht.

## Offen

Zurzeit nichts.

## Später (Optimierungen)

- [ ] Chunk-genaues inkrementelles Rendern — **zurückgestellt, lohnt sich zurzeit nicht** (Messung 2026-11-13)
  - Messung an der echten Welt (`data/server`, 34 Overworld-Regionen, Zeitstempel-Tabellen der `.mca`):
    die letzten Speichervorgänge schrieben 101 Chunks in 2 Regionen, 262 in 4 und 1254 in 4 Regionen.
    Neu zu berechnen wären davon (Rand 3 Chunks bei `reach` 32) 17–46 % pro betroffener Region,
    einzulesen 40–90 % (inkl. Nachbarregionen). Geschätzte Ersparnis ~40–60 % pro Region.
  - Absolut: 4 Regionen neu rendern = 3,9 s mit 1 Thread, 2,5 s mit 2 (`MAP_THREADS`-Default), ohne Änderung 33 ms;
    volle Welt 17,5 s. Ein Autosave (alle 5 min) kostet also ~1–2,5 s CPU bei niedriger Priorität.
    Die Ersparnis (~1 s pro Autosave) rechtfertigt das Flicken von `v/`/`c/`/`b/`/`p/`/Köpfen/Decor nicht.
  - Wieder aufgreifen, wenn Welten deutlich größer/dichter werden oder viele Server gleichzeitig rendern.
  - Messung (2026-11-10, 1 Thread, volle Region): Chunks lesen ~25 %, 2D-Kacheln ~3 %, 3D-Stapel (`voxels`) ~70 %.
    Nur die 2D-Kacheln chunkweise zu flicken spart daher fast nichts.
  - Approach: Für die 3D-Stapel hängt eine Spalte von allem in `reach + 1` Spalten ab: neu zu berechnen ist
    die Menge der geänderten Chunks + `ceil((reach+1)/16)` Chunks Rand, einzulesen davon nochmal so viel Rand;
    alte `v/`/`c/`-Datei dekodieren, die Spalten ersetzen, neu kodieren. Lohnt nur bei wenigen geänderten Chunks;
    um einen Spieler herum speichert Minecraft per Random Ticks (Wachstum, Gras) meist ~17×17 Chunks, plus Rand
    ist das fast die ganze Region. Vorher prüfen, wie viele Chunks pro Autosave wirklich neu geschrieben werden.
