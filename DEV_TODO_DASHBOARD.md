# DEV TODO – Dashboard (Panel)

Punkte zum Panel (und zum Projekt allgemein), die Input/Entscheidungen vom Entwickler brauchen.
Code-Aufgaben: [TODO_DASHBOARD.md](TODO_DASHBOARD.md). Live-Map-Themen: [DEV_TODO_LIVEMAP.md](DEV_TODO_LIVEMAP.md).

## Bitte testen

- [ ] Datapack „Todesbote“ (`deaths`): auf verschiedene Arten sterben (Fallen, Lava, Ertrinken, Zombie,
  Creeper, Pfeil …) → alle sehen einen Spruch passend zur Ursache, du zusätzlich deinen Todesort; Koordinaten
  anklicken = kopiert, `[TP]` = Teleport-Befehl in der Chatzeile. Eigene Sprüche: `datapacks/tools/gen_deaths.py`.
- [ ] Datapack „Nachtbilanz“ (`nightly`): eine Nacht spielen → Bilanz bei Sonnenaufgang; verschlafene Nacht →
  Bilanz trotzdem; ruhige Nacht → keine Meldung; `/trigger nachtbilanz` zeigt die letzte Bilanz + eigene Werte.
- [ ] Sprachen: Standard Englisch, `/trigger deutsch` / `/trigger english` — bei Todesbote, Nachtbilanz und
  Grabsteinen beide prüfen.
- [ ] Panel: Datapacks mit Bild, Namen und Kategorien — braucht den neu gebauten Container
  (`docker compose up -d --build`).

## Datapack-Ideen (besprechen)

Umgesetzt wird erst, was hier geklärt ist. Nächstes Pack: das Event-Pack; danach Mob-Jagd, Elite-Mobs,
verfluchte Truhen, Villager, Strukturen.

- [ ] Server-Events (Event-Pack) — Entscheidungen 2026-11-13:
  - ~jede 5. Nacht ein Event (Zufall ~20 %), halb gut/halb schlecht, ab 1 Spieler online; bei schlechten
    Nacht-Events ist Schlafen gesperrt, positive darf man verschlafen. Nie zwei gleichzeitig; Ankündigung per
    Titel + Bossbar; Admin-Befehl zum Erzwingen/Abschalten (evtl. Knopf im Panel).
  - Erste Version: Blutmond + Nebelnacht (schlecht), doppelte XP + goldene Ernte (gut).
    Blutmond „mittel“: ~doppelt so viele Mobs, etwas Ausrüstung/Tempo, doppelte Beute + XP.
    Nebelnacht: Dunkelheits-Effekt in Wellen, leise Höhlengeräusche, etwas mehr Mobs.
  - Später: Hühner-Regen, Elite-Mob-Nacht, verfluchte Woche, Glücksstunde beim Angeln, Meteoriteneinschlag,
    Sternschnuppennacht, wandernder Händler mit Spezialangebot.
  - Roter Himmel beim Blutmond: ja, als optionales Ressourcenpaket (wer ablehnt, sieht alles außer der
    Himmelsfarbe). Technisch zu prüfen: wie das Paket den Himmel nur in Blutmond-Nächten rot schaltet (Shader
    braucht ein Signal aus dem Spiel; 26.x Environment Attributes/Timelines ansehen) und wo es gehostet wird
    (nicht auf dem Map-Port, Panel-Port braucht Login → eigener öffentlicher Pfad oder GitHub-Release;
    `resource-pack` + `resource-pack-sha1` in server.properties). Kann auch nach der ersten Version kommen.
  - Nachtbilanz-Zähler (`nightly.*`) mitbenutzen (Blutmond-Bilanz am Morgen).
- [ ] Mob-Jagd mit Scoreboard — Entscheidungen 2026-11-13:
  - Runde = 3 Ingame-Tage (reines Datapack), Ziel-Mob per Zufall, nie zweimal hintereinander, und passend zum
    Fortschritt des Servers: Nether-Mobs erst, wenn jemand im Nether war (Advancement
    `minecraft:story/enter_the_nether`), End-Mobs erst nach dem End (`minecraft:story/enter_the_end`).
  - Wertung einzeln nach Kills des Ziel-Mobs.
  - Anzeige: Sidebar-Scoreboard, Chat-Ansagen (Führungswechsel, Rundenende), Live-Map (Ruhmeshalle, braucht
    Panel/Live-Map-Code). Sidebar muss jeder für sich ausblenden können (`/trigger jagd_aus`; technisch über
    Team-Sidebar `sidebar.team.<farbe>` — prüfen, ob sich das mit anderen Team-Nutzungen beißt).
  - Belohnung: Trophäe als „Wanderkrone“, die man im Spiel auf dem Kopf sieht und die zum nächsten Sieger
    wandert. Ohne Ressourcenpaket machbar als Spielerkopf mit Kronen-Textur (Base64-Texture); trägt Rundennummer
    in `custom_data`, alte Krone verschwindet beim Vorbesitzer (Inventar/Kopf; in Truhen beim Aufheben ersetzen).
  - Gleichstand: wer den Stand zuerst erreicht hat gewinnt; mindestens 3 Kills, sonst kein Sieger (Krone bleibt).
  - Ein Elite-Mob des Ziel-Typs zählt 5 Punkte; Blutmond zählt nicht extra.
- [ ] Elite-Mobs — Entscheidungen 2026-11-13:
  - ~1 von 200 feindlichen Mobs, 3× Leben, Ausrüstung, Bossbar im Kampf, gute Beute.
  - Typen: Zombie-Familie, Skelett-Familie, Creeper (größere Explosion) & Spinnen, Nether-Mobs (erst nach Nether).
  - Namen: witzige Liste („Gerd der Unkaputtbare“, Insider ergänzbar) — und Nemesis-Namen: wurde ein Spieler
    oft von diesem Mob-Typ getötet (Scoreboard `minecraft.killed_by:minecraft.<mob>`, z. B. ≥ 5), heißt ein
    Elite-Mob in seiner Nähe „<Spieler>s Albtraum“.
  - Chat-Ansage beim Sieg („Willi hat Gerd den Unkaputtbaren besiegt!“); zählt 5 Punkte in der Mob-Jagd.
- [ ] Verfluchte Truhen — Entscheidungen 2026-11-13:
  - Alle Struktur-Truhen, ~10 %, nur beim ersten Öffnen: Mob-Welle passend zum Ort (3–5 Mobs der Umgebung,
    Wüste: Husks, Dschungel: Spinnen, …), dafür Bonus-Beute in der Truhe.
  - Umsetzung: Advancement `player_generates_container_loot` (pro Loot-Table) → Zufall → Mobs um den Spieler/
    die Truhe; Bonus-Beute über eine eigene Loot-Table, die an die Vanilla-Tabellen angehängt wird.
- [ ] Villager-Verbesserungen, inkl. wandernder Händler mit Spezialangebot:
  - Entschieden 2026-11-13: Der wandernde Händler bietet zusätzlich seltene Blöcke (Schwamm, Sniffer-Ei,
    Budding Amethyst, Korallen, …) und Mob-Köpfe an. Villager bekommen neue Handelsangebote bei bestehenden
    Berufen. (Leine, Folgen, Preisdeckel: nicht gewünscht.)
  - Technik: 26.x hat datengetriebene Handelsangebote — prüfen, ob sich Villager-/Händler-Trades per Datapack
    ergänzen lassen; sonst beim Spawn per `data modify entity … Offers.Recipes append`.
  - Offen: Preise, wie viele Spezialangebote pro Händler.
- [ ] Eigene Advancements für den Server — Entscheidungen 2026-11-13:
  - Arten: zu unseren Packs (Ernte, Blutmond überlebt, Jagdkönig, Elite-Mob besiegt, …), Sammeln & Entdecken
    (alle Biome, Mob-Köpfe, Musik-Discs, Strukturen), lustige Pannen (eigener Pfeil, 10× Lava, Huhn, …).
    Keine Spielzeit-Abzeichen.
  - Eigener Server-Tab mit eigenem Icon/Hintergrund.
  - Ruhmeshalle: nicht in die Tier-Liste, eigene Rubrik → Aufgabe für die Live-Map-Seite (`leaderboard.rs`
    muss den Namespace der Server-Advancements getrennt zählen), in TODO_LIVEMAP.md eintragen, wenn das Pack steht.
  - Offen: konkrete Liste der Advancements (Namen, Texte, Bedingungen) — beim Bau sammeln.
- [ ] Dungeons / Gebäude in der Weltgenerierung — Entscheidungen 2026-11-13:
  - Arten: Ruinen & kleine Bauten (Wachturm, verlassenes Lager, Brunnen, Brücke), eigene Dungeons
    (Verliese mit Spawnern, Fallen, Beute), Dörfer verbessern (zusätzliche Häuser/Teile in den Village-Pools).
  - Claude generiert die Strukturen per Skript als NBT-Vorlagen (einfacher, sauberer Stil); Jigsaw-Strukturen,
    Structure-Sets, Biome-Tags, eigene Loot-Tables.
  - Dörfer: Vanilla-`template_pool`s überschreiben ersetzt sie → die Vanilla-Einträge aus dem Client-Jar übernehmen
    und eigene Teile anhängen (bei Minecraft-Updates neu abgleichen).
  - Wirkt nur in neu erzeugten Chunks. Offen: Häufigkeit, Schwierigkeit der Dungeons.
- [ ] Weitere Ideen sammeln.
