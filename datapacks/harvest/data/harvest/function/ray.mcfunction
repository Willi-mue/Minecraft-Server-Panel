# Ripe crop: harvest it. Unripe crops are low, so the line of sight passes over them to the ripe
# ones behind (otherwise a freshly replanted row would block the rest of the field).
execute if block ~ ~ ~ #harvest:crops align xyz positioned ~0.5 ~0.3 ~0.5 if function harvest:crop run return 1
execute unless block ~ ~ ~ #harvest:crops unless block ~ ~ ~ #harvest:see_through run return 0
scoreboard players remove @s harvest.tmp 1
execute if score @s harvest.tmp matches 1.. positioned ^ ^ ^0.1 run return run function harvest:ray
return 0
