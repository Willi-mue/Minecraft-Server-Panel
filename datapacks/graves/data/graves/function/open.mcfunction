# As the clicked grave's interaction entity. Only the owner may open it.
execute store result score #id graves.tmp run scoreboard players get @s graves.id
data modify storage graves:tmp owner set value [I; 0, 0, 0, 0]
execute as @e[type=minecraft:marker, tag=graves.grave, distance=..1] if score @s graves.id = #id graves.tmp run data modify storage graves:tmp owner set from entity @s data.owner
execute store success score #other graves.tmp run data modify storage graves:tmp owner set from entity @s interaction.player
data remove entity @s interaction
execute if score #other graves.tmp matches 1 run tellraw @a[tag=graves.clicker, scores={lang.de=0}] {text: "This is not your grave.", color: "gray"}
execute if score #other graves.tmp matches 1 run tellraw @a[tag=graves.clicker, scores={lang.de=1}] {text: "Das ist nicht dein Grab.", color: "gray"}
execute if score #other graves.tmp matches 1 run return 0
execute as @e[type=minecraft:marker, tag=graves.grave, distance=..1] if score @s graves.id = #id graves.tmp run data modify storage graves:tmp items set from entity @s data.items
execute at @e[tag=graves.clicker, limit=1] run function graves:spawn_items
# Nothing may get lost: if the items could not be handed out, they stay in the grave
execute if data storage graves:tmp items[0] run return 0
# Armor goes back on (the drops do not remember their slots), the rest can be picked up
execute as @e[tag=graves.clicker, limit=1] at @s run function graves:wear
execute as @e[type=minecraft:item, tag=graves.back] run data merge entity @s {PickupDelay: 0s}
tag @e[type=minecraft:item, tag=graves.back] remove graves.back
execute as @e[tag=graves.part, distance=..3] if score @s graves.id = #id graves.tmp run kill @s
playsound minecraft:block.chest.open block @a ~ ~ ~ 0.8 0.8
tellraw @a[tag=graves.clicker, scores={lang.de=0}] {text: "⚰ Your items are back.", color: "gray"}
tellraw @a[tag=graves.clicker, scores={lang.de=1}] {text: "⚰ Deine Items sind zurück.", color: "gray"}
