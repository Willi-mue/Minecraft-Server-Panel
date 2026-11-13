# Drops exactly what breaking the crop with the held hoe would give (Fortune counts). Replants
# only with a seed from the player's inventory; without one the crop is just harvested.
loot spawn ~ ~ ~ mine ~ ~ ~ mainhand
$execute store result score #seed harvest.tmp run function harvest:seed {seed: "$(seed)"}
$execute if score #seed harvest.tmp matches 1 run setblock ~ ~ ~ minecraft:$(block)[age=0]
execute if score #seed harvest.tmp matches 0 run setblock ~ ~ ~ minecraft:air
$playsound minecraft:block.$(sound).break block @a ~ ~ ~ 1 1
$execute if score #seed harvest.tmp matches 1 run playsound minecraft:item.$(sound).plant block @a ~ ~ ~ 0.6 1
particle minecraft:happy_villager ~ ~0.2 ~ 0.3 0.2 0.3 0 6
scoreboard players set @s harvest.cd 4
return 1
