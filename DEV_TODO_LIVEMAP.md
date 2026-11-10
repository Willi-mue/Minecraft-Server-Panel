# DEV TODO – Live-Map

Punkte zur Live-Map, die Input/Entscheidungen vom Entwickler brauchen. Code-Aufgaben: [TODO_LIVEMAP.md](TODO_LIVEMAP.md).
Panel-Themen: [DEV_TODO_DASHBOARD.md](DEV_TODO_DASHBOARD.md).

## Bitte testen (braucht echten Minecraft-Server, also akzeptierte EULA)

- [ ] Neue eigene Live-Map (Rust, Stufe 1)
  - Needs from you: Nach `docker compose up -d --build` `http://<host>:47319` öffnen: Die Karte erscheint sofort (auch bei gestopptem Server). Dann Server starten, im Spiel herumlaufen: Dein Name sollte nach wenigen Sekunden als Marker erscheinen (auch bei Vanilla). Neue Gebiete erscheinen nach dem nächsten Autosave (bis ca. 5 Min.) oder sofort mit dem Pfeil-Button neben "Open map" (speichert die Welt und rendert). Beim ersten Start wird das alte squaremap-Plugin automatisch entfernt. Gefallen dir Farben/Schattierung?

- [ ] 3D-Ansicht der Live-Map
  - Needs from you: Karte öffnen, oben links auf "3D" klicken. Linke Maustaste verschiebt, rechte (oder Strg + linke) schaut dich um wie ein Spieler (die Kamera dreht sich um sich selbst), Mausrad zoomt (zum Mauszeiger). Läuft es auf deinem Laptop flüssig, auch weit rausgezoomt? Gefällt dir die Block-Darstellung (nah jeder Block einzeln, weiter weg größere Blöcke)? Getestet habe ich nur mit Software-Rendering (headless Edge).

- [ ] Höhlen, Höhenschnitt, Flug und Köpfe in 3D
  - Needs from you: Vorher neu bauen (`docker compose up -d --build`, mapgen hat sich geändert; der erste Render danach rendert alles neu). In 3D in Schluchten und Höhleneingänge schauen: Wände sollten echte Blöcke sein, die Höhle nach ca. 32 Blöcken mit einer schlichten Felswand enden, keine Löcher. Höhenschnitt über „Slice“ am rechten Rand einschalten: das Band ziehen, seine Enden ziehen (Höhe 4–64) oder darüber scrollen. Optional mit `MAP_ALL_CAVES=true` (oder `PUT /api/map/visibility` `{"all_caves": true}`): dann zeigt das Band **alle** Höhlen (eigene Dateien `c/`). Fliegen: mehrere Tasten gleichzeitig (z. B. W+D+Leertaste), W fliegt dorthin, wo die Kamera hinschaut; Mausrad *während* eine Flugtaste gedrückt ist ändert die Geschwindigkeit (Anzeige unten in der Mitte). Nach Neuladen der Seite sollten Einstellungen, Geschwindigkeit und letzte Ansicht erhalten sein. Erscheint dein Skin-Kopf auf der Karte?

## Entscheidungen

- [ ] Anpassungen der Karte
  - Needs from you: Was soll an der Karte gestaltbar sein? Ideen: eigene Farben/Logo/Titel, Marker (Spawn, Basen, Warps), Startansicht und Zoom, Ebenen ein-/ausblendbar, Biom-Färbung, Sichtbarkeit von Spielern (z. B. ausblenden, nur Name), Karte nur mit Passwort. "Vieles, besprechen wir später."

- [ ] Wegpunkte: Zuordnung über IP und Schreibzugriff auf dem Map-Port
  - Hintergrund: Code-Aufgabe in TODO_LIVEMAP („Wegpunkte auf der Karte“). Die IP-Zuordnung ist unsicher: Mehrere Spieler im selben WLAN haben dieselbe IP, IPs wechseln, und wer die IP eines Spielers nutzt (z. B. gleiches Netz), kann als dieser Spieler Wegpunkte setzen. Außerdem darf der Map-Port bisher nur lesen.
  - Needs from you:
    - Reicht die IP-Zuordnung, oder soll es zusätzlich eine Bestätigung im Spiel geben (z. B. Code auf der Karte, den man im Chat eingibt)?
    - Läuft die Karte hinter einem Reverse-Proxy (dann kommt die echte IP über `X-Forwarded-For`)? Welche Proxy-Adresse ist vertrauenswürdig?
    - Wie lange gilt eine IP als „zu diesem Spieler gehörend“ (z. B. letzte 30 Tage)?
    - Dürfen Spieler ihre eigenen Wegpunkte bearbeiten/löschen, oder nur OPs? Private Wegpunkte (nur für einen selbst sichtbar)?

- [ ] Live-Map aus dem Internet erreichbar?
  - Needs from you: Soll die Karte öffentlich erreichbar sein (z. B. `map.deinedomain.de` hinter demselben Reverse-Proxy wie das Panel)?
