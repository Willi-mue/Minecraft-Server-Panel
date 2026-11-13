# Hoes cannot be "used" in vanilla, so right-clicking a crop triggers nothing a data pack can see.
# While a player looks at a crop with a hoe in the main hand, the hoe gets a consumable component
# that never finishes (no sound, no particles, no slowdown): the right-click becomes "using the
# item", which fires the using_item advancement every tick. Looking elsewhere removes it again,
# so shields, off-hand torches and food keep working as usual.
execute unless data storage harvest:state {off: 1b} as @a[gamemode=!spectator] if items entity @s weapon.mainhand #minecraft:hoes at @s anchored eyes positioned ^ ^ ^ run function harvest:aim
execute as @a if items entity @s weapon.offhand #minecraft:hoes[minecraft:consumable] run item modify entity @s weapon.offhand harvest:disarm
scoreboard players remove @a[scores={harvest.cd=1..}] harvest.cd 1
