# Right-clicking a stair with an empty hand does nothing in vanilla, so no trigger fires. Instead,
# while a player looks at a free seat with an empty main hand, an invisible interaction entity lies
# over its seat surface; clicking it fires player_interacted_with_entity. Targets are kept as long
# as somebody looks at them (a new entity every tick would swallow the click).
tag @e[type=minecraft:interaction, tag=sit.target] remove sit.keep
tag @e[tag=sit.riding] remove sit.riding
execute as @e[type=minecraft:item_display, tag=sit.seat] on passengers run tag @s add sit.riding
scoreboard players add @a sit.cd 0
execute as @a[gamemode=!spectator, tag=!sit.riding, scores={sit.cd=..0}] unless items entity @s weapon.mainhand * at @s anchored eyes positioned ^ ^ ^ run function sit:aim
# Hit with a fist: the player wants to break the block, so step aside for 2 seconds
execute as @e[type=minecraft:interaction, tag=sit.target] if data entity @s attack at @s run scoreboard players set @p sit.cd 40
execute as @e[type=minecraft:interaction, tag=sit.target] if data entity @s attack run tag @s remove sit.keep
kill @e[type=minecraft:interaction, tag=sit.target, tag=!sit.keep]
scoreboard players remove @a[scores={sit.cd=1..}] sit.cd 1
# Seats nobody sits on any more (sneaked off) or whose block is gone
tag @e[type=minecraft:item_display, tag=sit.seat] add sit.empty
execute as @e[type=minecraft:item_display, tag=sit.seat] on passengers on vehicle run tag @s remove sit.empty
execute as @e[type=minecraft:item_display, tag=sit.seat] at @s unless function sit:valid run tag @s add sit.empty
execute as @e[type=minecraft:item_display, tag=sit.seat, tag=sit.empty] at @s run function sit:stand_up
