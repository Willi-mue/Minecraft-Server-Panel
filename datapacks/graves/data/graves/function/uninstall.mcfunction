# Before switching the pack off: every loaded grave drops its items where it is (they then
# despawn after 5 minutes like normal drops), and the graves disappear
execute as @e[type=minecraft:marker, tag=graves.grave] at @s run function graves:spill
tag @e[type=minecraft:item, tag=graves.back] remove graves.back
kill @e[tag=graves.part]
tellraw @s "Graves: loaded graves emptied. You can switch the pack off now."
