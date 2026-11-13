# Invisible frames stay invisible without the pack (that is plain vanilla data). Run this before
# switching the pack off to make all loaded ones visible again; their panes drop.
execute as @e[type=#frames:frames, nbt={Invisible: 1b}] run data merge entity @s {Invisible: 0b}
execute as @e[type=minecraft:marker, tag=frames.pane] at @s run function frames:drop_pane
tellraw @s "Frames: geladene Rahmen wieder sichtbar. Jetzt kann das Pack aus."
