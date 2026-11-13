scoreboard players set #night nightly.tmp 0
execute unless entity @a run return 0
scoreboard players add @a nightly.kills 0
scoreboard players add @a nightly.deaths 0
scoreboard players add @a nightly.walk 0
scoreboard players add @a nightly.sprint 0
# Distance on foot in blocks (walking + sprinting, counted in cm)
execute as @a run scoreboard players operation @s nightly.walk += @s nightly.sprint
execute as @a run scoreboard players operation @s nightly.walk /= #100 nightly.tmp
# Everybody's own values, kept for /trigger nachtbilanz until the next dawn
scoreboard players reset * nightly.lk
scoreboard players reset * nightly.ld
scoreboard players reset * nightly.lw
execute as @a run scoreboard players operation @s nightly.lk = @s nightly.kills
execute as @a run scoreboard players operation @s nightly.ld = @s nightly.deaths
execute as @a run scoreboard players operation @s nightly.lw = @s nightly.walk
scoreboard players set #kills nightly.tmp 0
scoreboard players set #deaths nightly.tmp 0
scoreboard players set #walk nightly.tmp 0
scoreboard players operation #kills nightly.tmp > @a nightly.kills
scoreboard players operation #deaths nightly.tmp > @a nightly.deaths
scoreboard players operation #walk nightly.tmp > @a nightly.walk
data remove storage nightly:last r
data modify storage nightly:last r.done set value 1b
# Nothing happened (everybody slept or stood still): no message
execute if score #kills nightly.tmp matches 0 if score #deaths nightly.tmp matches 0 if score #walk nightly.tmp matches ..9 run return 0
tellraw @a[scores={lang.de=0}] [{text: "☀ ", color: "gold"}, {text: "The night is over! Report of the night:", color: "yellow"}]
tellraw @a[scores={lang.de=1}] [{text: "☀ ", color: "gold"}, {text: "Die Nacht ist vorbei! Bilanz der Nacht:", color: "yellow"}]
execute if score #kills nightly.tmp matches 1.. run function nightly:line {key: "kills", objective: "nightly.kills", max: "#kills", icon: "⚔", en: "Most mobs defeated", de: "Meiste Mobs besiegt", unit_en: "", unit_de: ""}
execute if score #deaths nightly.tmp matches 1.. run function nightly:line {key: "deaths", objective: "nightly.deaths", max: "#deaths", icon: "☠", en: "Died most often", de: "Am häufigsten gestorben", unit_en: "×", unit_de: "×"}
execute if score #walk nightly.tmp matches 10.. run function nightly:line {key: "walk", objective: "nightly.walk", max: "#walk", icon: "➜", en: "Walked the farthest", de: "Am weitesten gelaufen", unit_en: " blocks", unit_de: " Blöcke"}
