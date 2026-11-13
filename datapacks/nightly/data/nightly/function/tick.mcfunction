scoreboard players add @a lang.de 0
scoreboard players enable @a deutsch
scoreboard players enable @a english
execute as @a[scores={deutsch=1..}] run function nightly:lang {de: 1}
execute as @a[scores={english=1..}] run function nightly:lang {de: 0}
# Night = overworld clock between 13000 and 23000. Sleeping skips to the morning: also a dawn.
execute store result score #now nightly.tmp if predicate nightly:night
execute if score #now nightly.tmp matches 1 unless score #night nightly.tmp matches 1 run function nightly:dusk
execute if score #now nightly.tmp matches 0 if score #night nightly.tmp matches 1 run function nightly:dawn
scoreboard players enable @a nachtbilanz
scoreboard players enable @a lastnight
execute as @a[scores={nachtbilanz=1..}] run function nightly:show
execute as @a[scores={lastnight=1..}] run function nightly:show
