$execute if items entity @s $(slot) * run return 0
$execute as @e[type=minecraft:item, tag=graves.back, distance=..3] if items entity @s contents $(items) run tag @s add graves.cand
execute as @e[type=minecraft:item, tag=graves.cand, limit=1] run tag @s add graves.wear
$item replace entity @s $(slot) from entity @e[type=minecraft:item, tag=graves.wear, limit=1] contents
kill @e[type=minecraft:item, tag=graves.wear]
tag @e[type=minecraft:item, tag=graves.cand] remove graves.cand
