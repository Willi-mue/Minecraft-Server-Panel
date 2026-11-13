# The click itself rotates the frame's item (or puts the pane into an empty frame) before a data
# pack hears of it, so remember the state of the frame a sneaking player with a glass pane aims at.
tag @e[type=#frames:frames, tag=frames.aim] remove frames.aim
execute as @a[gamemode=!spectator, predicate=frames:sneaking] if items entity @s weapon.mainhand minecraft:glass_pane at @s anchored eyes positioned ^ ^ ^ run function frames:aim
# An invisible frame that is empty would be impossible to find again
execute as @e[type=#frames:frames, nbt={Invisible: 1b}] unless data entity @s Item run data merge entity @s {Invisible: 0b}
# A marker sits on every frame that holds a pane (made invisible in survival). Frame broken or
# visible again: the pane drops out.
execute as @e[type=minecraft:marker, tag=frames.pane] at @s unless entity @e[type=#frames:frames, distance=..0.01, nbt={Invisible: 1b}] run function frames:drop_pane
