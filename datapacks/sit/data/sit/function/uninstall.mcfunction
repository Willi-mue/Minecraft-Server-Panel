# Before switching the pack off: everybody stands up, click targets go away
execute as @e[type=minecraft:item_display, tag=sit.seat] at @s run function sit:stand_up
kill @e[type=minecraft:interaction, tag=sit.target]
tellraw @s "Sit: Sitze entfernt. Jetzt das Pack ausschalten."
