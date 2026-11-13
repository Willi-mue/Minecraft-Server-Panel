# As the player who just died: a funny line for everybody, the place for the player
scoreboard players set @s deaths.n 0
execute unless score @s deaths.cause matches 1.. run scoreboard players set @s deaths.cause 0
execute if score @s deaths.cause matches 1 run function deaths:msg/fall
execute if score @s deaths.cause matches 2 run function deaths:msg/fire
execute if score @s deaths.cause matches 3 run function deaths:msg/lava
execute if score @s deaths.cause matches 4 run function deaths:msg/drowning
execute if score @s deaths.cause matches 5 run function deaths:msg/explosion
execute if score @s deaths.cause matches 6 run function deaths:msg/projectile
execute if score @s deaths.cause matches 7 run function deaths:msg/freezing
execute if score @s deaths.cause matches 8 run function deaths:msg/void
execute if score @s deaths.cause matches 9 run function deaths:msg/starve
execute if score @s deaths.cause matches 10 run function deaths:msg/prickly
execute if score @s deaths.cause matches 11 run function deaths:msg/zombie
execute if score @s deaths.cause matches 12 run function deaths:msg/player
execute if score @s deaths.cause matches 13 run function deaths:msg/warden
execute if score @s deaths.cause matches 14 run function deaths:msg/suffocation
execute if score @s deaths.cause matches 15 run function deaths:msg/elytra
execute if score @s deaths.cause matches 16 run function deaths:msg/magic
execute if score @s deaths.cause matches 0 run function deaths:msg/other
scoreboard players set @s deaths.cause 0
scoreboard players set @s deaths.t 0
function deaths:place
