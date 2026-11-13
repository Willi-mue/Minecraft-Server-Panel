execute store result score @s harvest.tmp run attribute @s minecraft:block_interaction_range get 10
execute store result score @s harvest.aim run function harvest:find
execute if score @s harvest.aim matches 1 if items entity @s weapon.mainhand #minecraft:hoes[!minecraft:consumable] run item modify entity @s weapon.mainhand harvest:arm
execute if score @s harvest.aim matches 0 if items entity @s weapon.mainhand #minecraft:hoes[minecraft:consumable] run item modify entity @s weapon.mainhand harvest:disarm
