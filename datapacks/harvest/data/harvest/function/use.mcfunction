# Runs as the player every tick while a hoe is being "used" (right mouse button held)
advancement revoke @s only harvest:use
execute unless items entity @s weapon.mainhand #minecraft:hoes run return 0
execute if entity @s[gamemode=adventure] run return 0
execute if score @s harvest.cd matches 1.. run return 0
# Follow the line of sight from the eyes in 0.1 block steps, as far as the player can reach
# (block interaction range: 4.5 survival, 5 creative)
execute store result score @s harvest.tmp run attribute @s minecraft:block_interaction_range get 10
execute at @s anchored eyes positioned ^ ^ ^ run function harvest:ray
