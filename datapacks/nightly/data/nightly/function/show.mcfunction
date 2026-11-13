# As the player who ran /trigger nachtbilanz (or lastnight)
scoreboard players set @s nachtbilanz 0
scoreboard players set @s lastnight 0
execute if score @s lang.de matches 1 run return run function nightly:show_de
execute unless data storage nightly:last r run return run tellraw @s {text: "No night report yet – the first one comes at the next sunrise.", color: "gray"}
tellraw @s [{text: "☾ ", color: "gold"}, {text: "Report of the last night:", color: "yellow"}]
execute if data storage nightly:last r.kills run tellraw @s [{text: "  ⚔ Most mobs defeated: ", color: "gray"}, {nbt: "r.kills.name", storage: "nightly:last", interpret: true, color: "gold"}, {text: " (", color: "gray"}, {nbt: "r.kills.n", storage: "nightly:last", color: "white"}, {text: ")", color: "gray"}]
execute if data storage nightly:last r.deaths run tellraw @s [{text: "  ☠ Died most often: ", color: "gray"}, {nbt: "r.deaths.name", storage: "nightly:last", interpret: true, color: "gold"}, {text: " (", color: "gray"}, {nbt: "r.deaths.n", storage: "nightly:last", color: "white"}, {text: "×)", color: "gray"}]
execute if data storage nightly:last r.walk run tellraw @s [{text: "  ➜ Walked the farthest: ", color: "gray"}, {nbt: "r.walk.name", storage: "nightly:last", interpret: true, color: "gold"}, {text: " (", color: "gray"}, {nbt: "r.walk.n", storage: "nightly:last", color: "white"}, {text: " blocks)", color: "gray"}]
execute unless data storage nightly:last r.kills unless data storage nightly:last r.deaths unless data storage nightly:last r.walk run tellraw @s {text: "  It was a quiet night.", color: "gray"}
execute if score @s nightly.lk matches 0.. run tellraw @s [{text: "  You: ", color: "gray"}, {score: {name: "@s", objective: "nightly.lk"}, color: "white"}, {text: " mobs, ", color: "gray"}, {score: {name: "@s", objective: "nightly.ld"}, color: "white"}, {text: " deaths, ", color: "gray"}, {score: {name: "@s", objective: "nightly.lw"}, color: "white"}, {text: " blocks walked", color: "gray"}]
