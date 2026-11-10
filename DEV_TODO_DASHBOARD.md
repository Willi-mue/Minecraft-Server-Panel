# DEV TODO – Dashboard (Panel)

Punkte zum Panel (und zum Projekt allgemein), die Input/Entscheidungen vom Entwickler brauchen.
Code-Aufgaben: [TODO_DASHBOARD.md](TODO_DASHBOARD.md). Live-Map-Themen: [DEV_TODO_LIVEMAP.md](DEV_TODO_LIVEMAP.md).

## Bitte testen (braucht echten Minecraft-Server, also akzeptierte EULA)

- [ ] Neue Welt
  - Needs from you: Server stoppen → Backups → "New world". Prüfen: `archive_…zip` liegt im Backup-Ordner, neue Welt wird beim Start erzeugt, Karte zeigt die neue Welt.

- [ ] Kompletter Ablauf im Docker-Container
  - Needs from you: Start → Status "running", Beitreten/Verlassen/Chat sichtbar, Live-Backup, Restart mit Countdown (nur mit Spieler online) und Abbrechen, Backup wiederherstellen, `docker compose down` → im Log steht "Saving chunks" bzw. "ThreadedAnvilChunkStorage: All dimensions are saved".

- [ ] Mehrere Server und neues Panel-Design
  - Needs from you: Panel neu starten (stoppt auch den laufenden Minecraft-Server). Seiten durchklicken, zweiten Server über „+ New server“ anlegen und parallel zum ersten starten. Im Router/in der Firewall den zusätzlichen Port (Standard 25566) freigeben, wenn Freunde von außen beitreten sollen. Bei Docker die neue Port-Zeile in `docker-compose.yml` übernehmen (Container neu erstellen).

- [ ] Neue Funktionen von 2026-11-10 mit laufendem Server
  - Needs from you: Panel neu starten (Python-Änderungen). Dann prüfen:
    - **Login bleibt erhalten**: einloggen, Panel neu starten, Seite neu laden → kein Login nötig.
    - **TPS**: Übersicht zeigt nach ~10 s einen TPS-Wert (Paper über `tps`/`mspt`, Vanilla/Fabric über `tick query`); die Abfragen dürfen **nicht** in der Konsole auftauchen. Nach ein paar Minuten erscheint die Karte „Last hour“.
    - **Game rules** (Welt & Backups): bei laufendem Server steht oben „Live from the running server“. Steht dort „Values as of the last world save“, hat Minecraft die `gamerule`-Antwort anders formuliert: bitte die Konsolenzeile von `/gamerule keep_inventory` schicken.
    - **Spielerprofil** (Namen anklicken): Spielmodus, „To spawn“, „Heal“, Inventar leeren, Kick, Bann mit Grund; Skin-Kopf erscheint.
    - **Fabric**: zweiten Server mit Fabric anlegen, starten (lädt beim ersten Start Minecraft + Libraries), unter Add-ons → Mods z. B. „Lithium“ installieren (Fabric API kommt als Abhängigkeit mit), Neustart.
    - **Plugins**: beim Paper-Server Add-ons → Plugins, z. B. „EssentialsX“ installieren, „Check for updates“.
    - **Welt hochladen**: einen Einzelspieler-Speicherstand als .zip hochladen, in der Backup-Liste „Restore“.

## Entscheidungen

- [ ] Panel ebenfalls auf Rust umbauen?
  - Stand 2026-11-09: Empfehlung **vorerst nicht**. Gemessen: Python-Anteil im Image ~48 MB von 736 MB (Java 351 MB), Panel ~47 MB RAM gegenüber 1–4 GB für Minecraft. Ein Umbau spart also ~1–4 % und kostet einen kompletten Neubau samt neuer Fehler, während das Panel noch wächst. Rust bleibt dort, wo gerechnet wird (Karte, 3D).
  - Needs from you: Später neu entscheiden, wenn der Funktionsumfang steht und ein einzelnes Programm ohne Python wichtig wird.
