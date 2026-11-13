# As the frame: visible again, its pane (if it holds one) goes back to the player
data merge entity @s {Invisible: 0b}
execute at @s if entity @e[type=minecraft:marker, tag=frames.pane, distance=..0.01] run give @p minecraft:glass_pane
execute at @s run kill @e[type=minecraft:marker, tag=frames.pane, distance=..0.01]
playsound minecraft:block.glass.break block @a ~ ~ ~ 0.6 1.4
