# At the bottom centre of a block: 1 = a stair or slab you can sit on (bottom half, room above)
execute unless block ~ ~ ~ #minecraft:stairs[half=bottom] unless block ~ ~ ~ #minecraft:slabs[type=bottom] run return 0
execute unless block ~ ~1 ~ #sit:see_through run return 0
return 1
