# Runs as the player who right-clicked an interaction entity (ours or someone else's)
advancement revoke @s only sit:interact
execute as @e[type=minecraft:interaction, tag=sit.target, distance=..8, sort=nearest] if data entity @s interaction run tag @s add sit.hit
execute at @e[type=minecraft:interaction, tag=sit.hit, limit=1, sort=nearest] positioned ~ ~0.02 ~ align xyz positioned ~0.5 ~ ~0.5 run function sit:sit
execute as @e[type=minecraft:interaction, tag=sit.hit] run data remove entity @s interaction
tag @e[type=minecraft:interaction, tag=sit.hit] remove sit.hit
