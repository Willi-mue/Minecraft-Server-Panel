"""Generates datapacks/deaths (Todesbote). Run from the repo root:
python -I datapacks/tools/gen_deaths.py datapacks/deaths

Messages exist in English (default) and German; players switch with /trigger deutsch and
/trigger english (shared `lang.de` score, see datapacks/README.md). Each message is a pair
(english, german); {p} is the player's name.
"""
import json
import os
import shutil
import sys

OUT = sys.argv[1]

# cause id, name, damage source predicate ("type" of the damage predicate), messages
CAUSES = [
    (1, "fall", {"tags": [{"id": "#minecraft:is_fall", "expected": True}]}, [
        ("{p} underestimated gravity.", "{p} hat die Schwerkraft unterschätzt."),
        ("{p} tried to fly. The ground disagreed.", "{p} wollte fliegen. Der Boden hatte andere Pläne."),
        ("{p} took the shortcut down.", "{p} hat den kürzesten Weg nach unten genommen."),
        ("New free-fall record: {p}!", "Neuer Rekord im Freifall: {p}!"),
        ("{p} forgot that water at the bottom helps.", "{p} hat vergessen, dass unten Wasser hilfreich wäre."),
    ]),
    (2, "fire", {"tags": [{"id": "#minecraft:is_fire", "expected": True}, {"id": "#deaths:lava", "expected": False}]}, [
        ("{p} just wanted to warm up a little.", "{p} wollte sich nur kurz die Hände wärmen."),
        ("{p} is well done now.", "{p} ist jetzt gut durch."),
        ("{p} took the campfire a bit too literally.", "{p} hat das Lagerfeuer zu wörtlich genommen."),
        ("Anyone smell barbecue? Oh, that's {p}.", "Riecht hier jemand Grillfleisch? Ach, das ist {p}."),
    ]),
    (3, "lava", {"tags": [{"id": "#deaths:lava", "expected": True}]}, [
        ("{p} took a lava bath. A bit too warm.", "{p} hat ein Bad in der Lava genommen. Etwas zu warm."),
        ("{p} just wanted to check the temperature.", "{p} wollte nur mal die Temperatur prüfen."),
        ("The lava says thanks for the gear, {p}.", "Die Lava bedankt sich für die Ausrüstung, {p}."),
        ("{p} is part of the underworld now.", "{p} ist jetzt ein Teil der Unterwelt."),
    ]),
    (4, "drowning", {"tags": [{"id": "#minecraft:is_drowning", "expected": True}]}, [
        ("{p} forgot how breathing works.", "{p} hat vergessen, wie Atmen geht."),
        ("{p} wanted to see how the fish live.", "{p} wollte den Fischen beim Wohnen zusehen."),
        ("{p} has gills now. A little too late.", "{p} hat jetzt Kiemen. Leider zu spät."),
        ("Blub. Blub. {p}.", "Blub. Blub. {p}."),
    ]),
    (5, "explosion", {"tags": [{"id": "#minecraft:is_explosion", "expected": True}]}, [
        ("Sssss … {p} didn't hear it coming.", "Sssss … {p} hat es nicht kommen hören."),
        ("{p} was taken apart.", "{p} wurde in Einzelteile zerlegt."),
        ("{p} got a crater as a souvenir.", "{p} hat jetzt einen Krater als Andenken."),
        ("A creeper just wanted to hug {p}.", "Ein Creeper wollte {p} nur umarmen."),
    ]),
    (6, "projectile", {"tags": [{"id": "#minecraft:is_projectile", "expected": True}]}, [
        ("{p} collected one arrow too many.", "{p} hat einen Pfeil zu viel gesammelt."),
        ("{p} was the perfect target.", "{p} war die perfekte Zielscheibe."),
        ("Bullseye! Sadly, {p} was the target.", "Volltreffer! Leider war {p} das Ziel."),
        ("{p} is trying on a pincushion costume.", "{p} probiert gerade ein Nadelkissen-Kostüm aus."),
    ]),
    (7, "freezing", {"tags": [{"id": "#minecraft:is_freezing", "expected": True}]}, [
        ("{p} is a popsicle now.", "{p} ist jetzt ein Eis am Stiel."),
        ("{p} should have worn the leather boots.", "{p} hätte doch die Lederstiefel anziehen sollen."),
        ("Brrr. {p} stays cool forever.", "Brrr. {p} bleibt jetzt für immer cool."),
    ]),
    (8, "void", {"tags": [{"id": "#deaths:void", "expected": True}]}, [
        ("{p} fell into the void. Say hi to nothing.", "{p} ist ins Nichts gefallen. Grüße ans Nichts."),
        ("{p} is looking for the bottom of the world. There is none.", "{p} sucht jetzt den Boden der Welt. Es gibt keinen."),
        ("{p} found the end of the world. Literally.", "{p} hat das Ende der Welt gefunden. Wortwörtlich."),
    ]),
    (9, "starve", {"tags": [{"id": "#deaths:starve", "expected": True}]}, [
        ("{p} skipped lunch. Forever.", "{p} hat das Mittagessen ausfallen lassen. Für immer."),
        ("{p} should have packed another loaf of bread.", "{p} hätte doch noch ein Brot einpacken sollen."),
        ("Diet slightly overdone, {p}.", "Diät leicht übertrieben, {p}."),
    ]),
    (10, "prickly", {"tags": [{"id": "#deaths:prickly", "expected": True}]}, [
        ("{p} tried to hug the cactus.", "{p} wollte den Kaktus umarmen."),
        ("{p} learned: cacti are not cuddly toys.", "{p} hat gelernt: Kakteen sind keine Kuscheltiere."),
        ("Poke. Poke. Poke. Bye, {p}.", "Pieks. Pieks. Pieks. Tschüss, {p}."),
    ]),
    (11, "zombie", {"tags": [{"id": "#minecraft:is_projectile", "expected": False}],
                    "source_entity": {"minecraft:entity_type": "#deaths:zombies"}}, [
        ("{p} was a tasty brain buffet.", "{p} war ein leckeres Gehirn-Buffet."),
        ("The zombies invited {p} to dinner. As the main course.", "Die Zombies haben {p} zum Essen eingeladen. Als Hauptgang."),
        ("{p} joined team undead.", "{p} ist jetzt im Team Untot."),
    ]),
    (12, "player", {"source_entity": {"minecraft:entity_type": "minecraft:player"}}, [
        ("{p} lost the argument.", "{p} hat den Streit verloren."),
        ("{p} was surprised by a friend. Some friend!", "{p} wurde von einem Freund überrascht. Was für ein Freund!"),
        ("{p} picked a fight with the wrong person.", "{p} hat sich mit dem Falschen angelegt."),
    ]),
    (13, "warden", {"source_entity": {"minecraft:entity_type": "minecraft:warden"}}, [
        ("{p} was too loud. The Warden doesn't like that.", "{p} war zu laut. Der Warden mag das nicht."),
        ("Shh, {p}! Too late.", "Psst, {p}! Zu spät."),
    ]),
    (14, "suffocation", {"tags": [{"id": "#deaths:suffocation", "expected": True}]}, [
        ("{p} wanted to know what it's like inside a block.", "{p} wollte wissen, wie es im Block aussieht."),
        ("{p} is stuck. Forever.", "{p} steckt fest. Für immer."),
    ]),
    (15, "elytra", {"tags": [{"id": "#deaths:elytra", "expected": True}]}, [
        ("{p} flew into a wall at full speed.", "{p} ist mit Vollgas gegen die Wand geflogen."),
        ("Braking would have helped, {p}.", "Bremsen hätte geholfen, {p}."),
    ]),
    (16, "magic", {"tags": [{"id": "#deaths:magic", "expected": True}]}, [
        ("{p} tried the wrong potion.", "{p} hat den falschen Trank probiert."),
        ("Witchcraft! {p} got hit.", "Hexerei! {p} hat es erwischt."),
    ]),
]
OTHER = [
    ("{p} passed away mysteriously.", "{p} ist auf mysteriöse Weise verschieden."),
    ("{p} kicked the bucket.", "{p} hat das Zeitliche gesegnet."),
    ("{p} is taking a short break. A very long one.", "{p} macht jetzt eine kleine Pause. Eine sehr lange."),
    ("Rest in peace, {p}. Or just respawn.", "Ruhe in Frieden, {p}. Oder einfach respawnen."),
    ("{p} booked a trip to the afterlife.", "{p} hat einen Ausflug ins Jenseits gebucht."),
]
DAMAGE_TAGS = {
    "lava": ["minecraft:lava"],
    "void": ["minecraft:out_of_world"],
    "starve": ["minecraft:starve"],
    "prickly": ["minecraft:cactus", "minecraft:sweet_berry_bush"],
    "suffocation": ["minecraft:in_wall"],
    "elytra": ["minecraft:fly_into_wall"],
    "magic": ["minecraft:magic", "minecraft:indirect_magic", "minecraft:wither"],
}
ZOMBIES = ["minecraft:zombie", "minecraft:husk", "minecraft:drowned", "minecraft:zombie_villager",
           "minecraft:zombified_piglin"]

EN = "@a[scores={lang.de=0}]"
DE = "@a[scores={lang.de=1}]"

# Shared by every pack with chat messages (identical in each, so any one of them handles it)
LANG_LOAD = """\
# Language of the chat messages: lang.de 0 = English (default), 1 = German; shared by all packs
scoreboard objectives add lang.de dummy
scoreboard objectives add deutsch trigger
scoreboard objectives add english trigger
"""
LANG_TICK = """\
scoreboard players add @a lang.de 0
scoreboard players enable @a deutsch
scoreboard players enable @a english
execute as @a[scores={deutsch=1..}] run function {ns}:lang {de: 1}
execute as @a[scores={english=1..}] run function {ns}:lang {de: 0}
"""
LANG_FN = """\
# /trigger deutsch | english (the same in every pack with chat messages)
scoreboard players set @s deutsch 0
scoreboard players set @s english 0
$scoreboard players set @s lang.de $(de)
tellraw @s[scores={lang.de=0}] {text: "Language: English. Type /trigger deutsch for German.", color: "gray"}
tellraw @s[scores={lang.de=1}] {text: "Sprache: Deutsch. Mit /trigger english zurück auf Englisch.", color: "gray"}
"""


def write(rel, text):
    path = os.path.join(OUT, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def jdump(obj):
    return json.dumps(obj, indent=2, ensure_ascii=False) + "\n"


def snbt_str(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def message(target, text):
    before, _, after = text.partition("{p}")
    parts = ['{text: "☠ ", color: "dark_gray"}']
    if before:
        parts.append(f'{{text: {snbt_str(before)}, color: "gray"}}')
    parts.append('{selector: "@s", color: "gold"}')
    if after:
        parts.append(f'{{text: {snbt_str(after)}, color: "gray"}}')
    return f"tellraw {target} [" + ", ".join(parts) + "]"


def msg_function(pairs):
    out = [f"execute store result score #r deaths.tmp run random value 1..{len(pairs)}"]
    for i, (en, de) in enumerate(pairs, 1):
        out.append(f"execute if score #r deaths.tmp matches {i} run {message(EN, en)}")
        out.append(f"execute if score #r deaths.tmp matches {i} run {message(DE, de)}")
    return "\n".join(out) + "\n"


if __name__ == "__main__":
    shutil.rmtree(os.path.join(OUT, "data"), ignore_errors=True)  # pack.png / pack.mcmeta stay

    write("data/minecraft/tags/function/load.json", jdump({"values": ["deaths:load"]}))
    write("data/minecraft/tags/function/tick.json", jdump({"values": ["deaths:tick"]}))
    for name, values in DAMAGE_TAGS.items():
        write(f"data/deaths/tags/damage_type/{name}.json", jdump({"values": values}))
    write("data/deaths/tags/entity_type/zombies.json", jdump({"values": ZOMBIES}))

    write("data/deaths/function/load.mcfunction", LANG_LOAD + """\
# Deaths since the last check / last damage cause and how long ago (ticks left) / scratch
scoreboard objectives add deaths.n deathCount
scoreboard objectives add deaths.cause dummy
scoreboard objectives add deaths.t dummy
scoreboard objectives add deaths.tmp dummy
""")
    write("data/deaths/function/tick.mcfunction", LANG_TICK.replace("{ns}", "deaths") + """\
# The cause of death is the last damage taken within 10 seconds (recorded by the hurt_* advancements)
scoreboard players remove @a[scores={deaths.t=1..}] deaths.t 1
scoreboard players set @a[scores={deaths.t=0}] deaths.cause 0
execute as @a[scores={deaths.n=1..}] run function deaths:died
""")
    write("data/deaths/function/lang.mcfunction", LANG_FN)
    dispatch = ["# As the player who just died: a funny line for everybody, the place for the player",
                "scoreboard players set @s deaths.n 0",
                "execute unless score @s deaths.cause matches 1.. run scoreboard players set @s deaths.cause 0"]
    for cid, name, _, _ in CAUSES:
        dispatch.append(f"execute if score @s deaths.cause matches {cid} run function deaths:msg/{name}")
    dispatch += [
        "execute if score @s deaths.cause matches 0 run function deaths:msg/other",
        "scoreboard players set @s deaths.cause 0",
        "scoreboard players set @s deaths.t 0",
        "function deaths:place",
    ]
    write("data/deaths/function/died.mcfunction", "\n".join(dispatch) + "\n")
    write("data/deaths/function/place.mcfunction", """\
execute unless data entity @s LastDeathLocation run return 0
data modify storage deaths:tmp a set value {en: "", de: ""}
data modify storage deaths:tmp a.x set from entity @s LastDeathLocation.pos[0]
data modify storage deaths:tmp a.y set from entity @s LastDeathLocation.pos[1]
data modify storage deaths:tmp a.z set from entity @s LastDeathLocation.pos[2]
data modify storage deaths:tmp a.dim set from entity @s LastDeathLocation.dimension
data modify storage deaths:tmp a.en set from entity @s LastDeathLocation.dimension
data modify storage deaths:tmp a.de set from entity @s LastDeathLocation.dimension
execute if data entity @s LastDeathLocation{dimension: "minecraft:overworld"} run data modify storage deaths:tmp a merge value {en: "Overworld", de: "Oberwelt"}
execute if data entity @s LastDeathLocation{dimension: "minecraft:the_nether"} run data modify storage deaths:tmp a merge value {en: "Nether", de: "Nether"}
execute if data entity @s LastDeathLocation{dimension: "minecraft:the_end"} run data modify storage deaths:tmp a merge value {en: "End", de: "End"}
function deaths:tell with storage deaths:tmp a
""")
    write("data/deaths/function/tell.mcfunction", """\
# Only the player who died sees where: click the coordinates to copy them, [TP] puts the teleport
# command into the chat line (works for operators)
$tellraw @s[scores={lang.de=0}] [{text: "☠ Where you died: ", color: "gray"}, {text: "$(x) $(y) $(z)", color: "yellow", click_event: {action: "copy_to_clipboard", value: "$(x) $(y) $(z)"}, hover_event: {action: "show_text", value: "Click to copy"}}, {text: " ($(en)) ", color: "gray"}, {text: "[TP]", color: "aqua", click_event: {action: "suggest_command", command: "/execute in $(dim) run tp @s $(x) $(y) $(z)"}, hover_event: {action: "show_text", value: "Puts the teleport command into the chat line (operators only)"}}]
$tellraw @s[scores={lang.de=1}] [{text: "☠ Dein Todesort: ", color: "gray"}, {text: "$(x) $(y) $(z)", color: "yellow", click_event: {action: "copy_to_clipboard", value: "$(x) $(y) $(z)"}, hover_event: {action: "show_text", value: "Klicken zum Kopieren"}}, {text: " ($(de)) ", color: "gray"}, {text: "[TP]", color: "aqua", click_event: {action: "suggest_command", command: "/execute in $(dim) run tp @s $(x) $(y) $(z)"}, hover_event: {action: "show_text", value: "Teleport-Befehl in die Chatzeile (nur für Ops)"}}]
""")
    for cid, name, source, pairs in CAUSES:
        write(f"data/deaths/advancement/hurt_{name}.json", jdump({
            "criteria": {"hurt": {"trigger": "minecraft:entity_hurt_player",
                                  "conditions": {"damage": {"type": source}}}},
            "rewards": {"function": f"deaths:cause/{name}"},
        }))
        write(f"data/deaths/function/cause/{name}.mcfunction",
              f"advancement revoke @s only deaths:hurt_{name}\n"
              f"scoreboard players set @s deaths.cause {cid}\n"
              f"scoreboard players set @s deaths.t 200\n")
        write(f"data/deaths/function/msg/{name}.mcfunction", msg_function(pairs))
    write("data/deaths/function/msg/other.mcfunction", msg_function(OTHER))
    write("data/deaths/function/uninstall.mcfunction", """\
scoreboard objectives remove deaths.n
scoreboard objectives remove deaths.cause
scoreboard objectives remove deaths.t
scoreboard objectives remove deaths.tmp
tellraw @s "Death messages: counters removed. You can switch the pack off now."
""")
    print("ok")
