# First frame within reach in the line of sight (blocks stop the search)
execute as @n[type=#frames:frames, distance=..0.5] run return run function frames:remember
execute unless block ~ ~ ~ #frames:see_through run return 0
scoreboard players remove @s frames.tmp 1
execute if score @s frames.tmp matches 1.. positioned ^ ^ ^0.1 run return run function frames:ray
return 0
