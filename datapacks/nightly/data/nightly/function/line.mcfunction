# Everybody with the best value gets named (ties: all of them)
$execute as @a if score @s $(objective) = $(max) nightly.tmp run tag @s add nightly.best
$tellraw @a[scores={lang.de=0}] [{text: "  $(icon) $(en): ", color: "gray"}, {selector: "@a[tag=nightly.best]", color: "gold"}, {text: " (", color: "gray"}, {score: {name: "$(max)", objective: "nightly.tmp"}, color: "white"}, {text: "$(unit_en))", color: "gray"}]
$tellraw @a[scores={lang.de=1}] [{text: "  $(icon) $(de): ", color: "gray"}, {selector: "@a[tag=nightly.best]", color: "gold"}, {text: " (", color: "gray"}, {score: {name: "$(max)", objective: "nightly.tmp"}, color: "white"}, {text: "$(unit_de))", color: "gray"}]
# Kept for /trigger nachtbilanz: the names as plain text (the players may be offline by then)
$execute as @a[tag=nightly.best, limit=1] at @s run function nightly:remember {key: "$(key)"}
$execute store result storage nightly:last r.$(key).n int 1 run scoreboard players get $(max) nightly.tmp
tag @a remove nightly.best
