# Runs as the player who right-clicked an interaction entity (any pack's)
advancement revoke @s only graves:interact
tag @s add graves.clicker
execute as @e[type=minecraft:interaction, tag=graves.click, distance=..8] if data entity @s interaction at @s run function graves:open
tag @s remove graves.clicker
