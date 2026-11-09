# DEV TODO – Dashboard (Panel)

Punkte zum Panel (und zum Projekt allgemein), die Input/Entscheidungen vom Entwickler brauchen.
Code-Aufgaben: [TODO_DASHBOARD.md](TODO_DASHBOARD.md). Live-Map-Themen: [DEV_TODO_LIVEMAP.md](DEV_TODO_LIVEMAP.md).

## Bitte testen (braucht echten Minecraft-Server, also akzeptierte EULA)

- [ ] Neue Welt
  - Needs from you: Server stoppen → Backups → "New world". Prüfen: `archive_…zip` liegt im Backup-Ordner, neue Welt wird beim Start erzeugt, Karte zeigt die neue Welt.

- [ ] Kompletter Ablauf im Docker-Container
  - Needs from you: Start → Status "running", Beitreten/Verlassen/Chat sichtbar, Live-Backup, Restart mit Countdown (nur mit Spieler online) und Abbrechen, Backup wiederherstellen, `docker compose down` → im Log steht "Saving chunks" bzw. "ThreadedAnvilChunkStorage: All dimensions are saved".

## Entscheidungen

- [ ] Panel ebenfalls auf Rust umbauen?
  - Stand 2026-11-09: Empfehlung **vorerst nicht**. Gemessen: Python-Anteil im Image ~48 MB von 736 MB (Java 351 MB), Panel ~47 MB RAM gegenüber 1–4 GB für Minecraft. Ein Umbau spart also ~1–4 % und kostet einen kompletten Neubau samt neuer Fehler, während das Panel noch wächst. Rust bleibt dort, wo gerechnet wird (Karte, 3D).
  - Needs from you: Später neu entscheiden, wenn der Funktionsumfang steht und ein einzelnes Programm ohne Python wichtig wird.
