# First block in the line of sight within reach; a free seat gets a click target
execute unless block ~ ~ ~ #sit:see_through align xyz positioned ~0.5 ~ ~0.5 if function sit:valid unless entity @e[type=minecraft:item_display, tag=sit.seat, distance=..0.6] run return run function sit:target
execute unless block ~ ~ ~ #sit:see_through run return 0
scoreboard players remove @s sit.tmp 1
execute if score @s sit.tmp matches 1.. positioned ^ ^ ^0.1 run return run function sit:ray
return 0
