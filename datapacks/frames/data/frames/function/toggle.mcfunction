# As the frame; @p is the clicking player. Was it empty, the click put the pane in: take it out
# again and give it back (an empty frame cannot be hidden). Otherwise undo the rotation and flip the
# visibility: hiding uses up one pane (not in creative), which the frame keeps until it is shown
# again (pane back to the player), emptied or broken (pane drops).
tag @s remove frames.aim
execute if entity @s[tag=frames.was_empty] run data remove entity @s Item
execute if entity @s[tag=frames.was_empty] as @p[gamemode=!creative] run give @s minecraft:glass_pane
execute if entity @s[tag=frames.was_empty] run return run tag @s remove frames.was_empty
execute store result entity @s ItemRotation byte 1 run scoreboard players get @s frames.rot
execute if data entity @s {Invisible: 1b} run return run function frames:show
data merge entity @s {Invisible: 1b}
execute at @s as @p[gamemode=!creative] if items entity @s weapon.mainhand minecraft:glass_pane run function frames:pay
playsound minecraft:block.glass.place block @a ~ ~ ~ 0.8 1.2
