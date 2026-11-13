# As the player, at the bottom centre of the seat block. The seat entity sits on the seat surface
# (half a block up) and faces away from the stair's backrest; on a slab it keeps the player's view.
execute if entity @s[tag=sit.riding] run return 0
execute unless function sit:valid run return 0
execute if entity @e[type=minecraft:item_display, tag=sit.seat, distance=..0.6] run return 0
summon minecraft:item_display ~ ~0.5 ~ {Tags: ["sit.seat", "sit.new"]}
execute if block ~ ~ ~ #minecraft:stairs[facing=north] run rotate @n[tag=sit.new] 0 0
execute if block ~ ~ ~ #minecraft:stairs[facing=south] run rotate @n[tag=sit.new] 180 0
execute if block ~ ~ ~ #minecraft:stairs[facing=east] run rotate @n[tag=sit.new] 90 0
execute if block ~ ~ ~ #minecraft:stairs[facing=west] run rotate @n[tag=sit.new] -90 0
execute if block ~ ~ ~ #minecraft:slabs at @s run rotate @n[tag=sit.new] ~ 0
ride @s mount @n[tag=sit.new]
execute at @n[tag=sit.new] run rotate @s ~ 0
tag @e[tag=sit.new] remove sit.new
