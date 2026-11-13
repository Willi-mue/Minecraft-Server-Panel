scoreboard players add @a lang.de 0
scoreboard players enable @a deutsch
scoreboard players enable @a english
execute as @a[scores={deutsch=1..}] run function graves:lang {de: 1}
execute as @a[scores={english=1..}] run function graves:lang {de: 0}
execute as @a[scores={graves.d=1..}] run function graves:died
