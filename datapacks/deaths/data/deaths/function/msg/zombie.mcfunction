execute store result score #r deaths.tmp run random value 1..3
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " was a tasty brain buffet.", color: "gray"}]
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " war ein leckeres Gehirn-Buffet.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {text: "The zombies invited ", color: "gray"}, {selector: "@s", color: "gold"}, {text: " to dinner. As the main course.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {text: "Die Zombies haben ", color: "gray"}, {selector: "@s", color: "gold"}, {text: " zum Essen eingeladen. Als Hauptgang.", color: "gray"}]
execute if score #r deaths.tmp matches 3 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " joined team undead.", color: "gray"}]
execute if score #r deaths.tmp matches 3 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " ist jetzt im Team Untot.", color: "gray"}]
