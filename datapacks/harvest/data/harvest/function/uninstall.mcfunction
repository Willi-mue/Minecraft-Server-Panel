# Run before switching the pack off: stops arming hoes and makes the ones in the players' hands
# normal again (a hoe elsewhere only keeps the extra component if it was armed when put away).
data modify storage harvest:state off set value 1b
execute as @a if items entity @s weapon.mainhand #minecraft:hoes run item modify entity @s weapon.mainhand harvest:disarm
execute as @a if items entity @s weapon.offhand #minecraft:hoes run item modify entity @s weapon.offhand harvest:disarm
tellraw @s "Harvest: Hacken zurückgesetzt. Jetzt das Pack ausschalten."
