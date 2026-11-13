scoreboard players add @a lang.de 0
scoreboard players enable @a deutsch
scoreboard players enable @a english
execute as @a[scores={deutsch=1..}] run function deaths:lang {de: 1}
execute as @a[scores={english=1..}] run function deaths:lang {de: 0}
# The cause of death is the last damage taken within 10 seconds (recorded by the hurt_* advancements)
scoreboard players remove @a[scores={deaths.t=1..}] deaths.t 1
scoreboard players set @a[scores={deaths.t=0}] deaths.cause 0
execute as @a[scores={deaths.n=1..}] run function deaths:died
