# DEV TODO – Live-Map

Punkte zur Live-Map, die Input/Entscheidungen vom Entwickler brauchen. Code-Aufgaben: [TODO_LIVEMAP.md](TODO_LIVEMAP.md).
Panel-Themen: [DEV_TODO_DASHBOARD.md](DEV_TODO_DASHBOARD.md).

## Bitte testen (braucht echten Minecraft-Server, also akzeptierte EULA)

- [ ] Neue eigene Live-Map (Rust, Stufe 1)
  - Needs from you: Nach `docker compose up -d --build` `http://<host>:47319` öffnen: Die Karte erscheint sofort (auch bei gestopptem Server). Dann Server starten, im Spiel herumlaufen: Dein Name sollte nach wenigen Sekunden als Marker erscheinen (auch bei Vanilla). Neue Gebiete erscheinen nach dem nächsten Autosave (bis ca. 5 Min.) oder sofort mit dem Pfeil-Button neben "Open map" (speichert die Welt und rendert). Beim ersten Start wird das alte squaremap-Plugin automatisch entfernt. Gefallen dir Farben/Schattierung?

- [ ] 3D-Ansicht der Live-Map
  - Needs from you: Karte öffnen, oben links auf "3D" klicken. Linke Maustaste verschiebt, rechte dreht, Mausrad zoomt (zum Mauszeiger). Läuft es auf deinem Laptop flüssig, auch weit rausgezoomt? Gefällt dir die Block-Darstellung (nah jeder Block einzeln, weiter weg größere Blöcke; Wasser ist eine flache Fläche ohne Grund)? Getestet habe ich nur mit Software-Rendering (headless Edge).

## Entscheidungen

- [x] Live-Map für alle Server-Typen: entschieden am 2026-11-09 → eigener Renderer in Rust, Karten-Item-Farben, Stufe 1 2D, Stufe 2 leichtes 3D (siehe TODO_LIVEMAP.md).
- [x] Echte Block-Texturen: entschieden am 2026-11-09 → wie BlueMap: Minecraft-Client wird zur Laufzeit von Mojang auf den eigenen Server geladen (nicht mit dem Projekt verteilt), Karten-Item-Farben nur noch als Rückfall.

- [ ] Anpassungen der Karte
  - Needs from you: Was soll an der Karte gestaltbar sein? Ideen: eigene Farben/Logo/Titel, Marker (Spawn, Basen, Warps), Startansicht und Zoom, Ebenen ein-/ausblendbar, Biom-Färbung, Sichtbarkeit von Spielern (z. B. ausblenden, nur Name), Karte nur mit Passwort. "Vieles, besprechen wir später."

- [ ] Live-Map aus dem Internet erreichbar?
  - Needs from you: Soll die Karte öffentlich erreichbar sein (z. B. `map.deinedomain.de` hinter demselben Reverse-Proxy wie das Panel)? Hängt mit "Zugriff aus dem Internet" in DEV_TODO_DASHBOARD zusammen.
