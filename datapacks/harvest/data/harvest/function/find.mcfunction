# 1 when the line of sight reaches a crop (ripe or not) within reach, else 0
execute if block ~ ~ ~ #harvest:crops run return 1
execute unless block ~ ~ ~ #harvest:see_through run return 0
scoreboard players remove @s harvest.tmp 1
execute if score @s harvest.tmp matches 1.. positioned ^ ^ ^0.1 run return run function harvest:find
return 0
