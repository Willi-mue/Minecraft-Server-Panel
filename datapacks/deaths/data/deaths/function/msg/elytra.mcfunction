execute store result score #r deaths.tmp run random value 1..2
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " flew into a wall at full speed.", color: "gray"}]
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " ist mit Vollgas gegen die Wand geflogen.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {text: "Braking would have helped, ", color: "gray"}, {selector: "@s", color: "gold"}, {text: ".", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {text: "Bremsen hätte geholfen, ", color: "gray"}, {selector: "@s", color: "gold"}, {text: ".", color: "gray"}]
