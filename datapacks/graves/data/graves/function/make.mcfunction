# As the dead player, at the death point. The marker keeps the owner and the items; the
# displays only show the grave, the interaction entity takes the click.
summon minecraft:marker ~ ~ ~ {Tags: ["graves.grave", "graves.part", "graves.new"], data: {items: []}}
data modify entity @n[type=minecraft:marker, tag=graves.new] data.owner set from entity @s UUID
# Freshly dropped items only (the death drops are at most a few ticks old)
execute as @e[type=minecraft:item, distance=..8, nbt={Age: 0s}] run function graves:take
execute as @e[type=minecraft:item, distance=..8, nbt={Age: 1s}] run function graves:take
execute as @e[type=minecraft:item, distance=..8, nbt={Age: 2s}] run function graves:take
execute as @e[type=minecraft:item, distance=..8, nbt={Age: 3s}] run function graves:take
# Nothing left (keepInventory, lava, void): no grave
execute unless data entity @n[type=minecraft:marker, tag=graves.new] data.items[0] run return run kill @e[type=minecraft:marker, tag=graves.new]
summon minecraft:block_display ~ ~ ~ {Tags: ["graves.part", "graves.new"], block_state: "minecraft:mossy_stone_bricks", transformation: {left_rotation: [0f, 0f, 0f, 1f], right_rotation: [0f, 0f, 0f, 1f], translation: [-0.35f, 0f, -0.08f], scale: [0.7f, 0.9f, 0.16f]}}
summon minecraft:item_display ~ ~1.15 ~ {Tags: ["graves.part", "graves.new"], item: {id: "minecraft:player_head", count: 1}, transformation: {left_rotation: [0f, 0f, 0f, 1f], right_rotation: [0f, 0f, 0f, 1f], translation: [0f, 0f, 0f], scale: [0.5f, 0.5f, 0.5f]}}
item modify entity @n[type=minecraft:item_display, tag=graves.new] contents graves:head
item modify entity @n[type=minecraft:item_display, tag=graves.new] contents graves:label
summon minecraft:text_display ~ ~1.5 ~ {Tags: ["graves.part", "graves.new"], billboard: "center", text: ""}
data modify entity @n[type=minecraft:text_display, tag=graves.new] text set from entity @n[type=minecraft:item_display, tag=graves.new] item.components."minecraft:custom_name"
summon minecraft:interaction ~ ~ ~ {Tags: ["graves.part", "graves.new", "graves.click"], width: 0.9f, height: 1.3f, response: 1b}
# Stone and head face the way the player looked
execute rotated as @s run tp @e[type=minecraft:block_display, tag=graves.new] ~ ~ ~ ~ 0
execute rotated as @s run tp @e[type=minecraft:item_display, tag=graves.new] ~ ~1.15 ~ ~ 0
scoreboard players add #next graves.id 1
scoreboard players operation @e[tag=graves.new] graves.id = #next graves.id
tag @e[tag=graves.new] remove graves.new
tellraw @s[scores={lang.de=0}] {text: "⚰ Your items are safe in your grave where you died. Right-click it to get them back.", color: "gray"}
tellraw @s[scores={lang.de=1}] {text: "⚰ Deine Items liegen sicher in deinem Grab am Todesort. Rechtsklick darauf holt sie zurück.", color: "gray"}
