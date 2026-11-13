execute store result score #r deaths.tmp run random value 1..4
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " forgot how breathing works.", color: "gray"}]
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " hat vergessen, wie Atmen geht.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " wanted to see how the fish live.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " wollte den Fischen beim Wohnen zusehen.", color: "gray"}]
execute if score #r deaths.tmp matches 3 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " has gills now. A little too late.", color: "gray"}]
execute if score #r deaths.tmp matches 3 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " hat jetzt Kiemen. Leider zu spät.", color: "gray"}]
execute if score #r deaths.tmp matches 4 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {text: "Blub. Blub. ", color: "gray"}, {selector: "@s", color: "gold"}, {text: ".", color: "gray"}]
execute if score #r deaths.tmp matches 4 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {text: "Blub. Blub. ", color: "gray"}, {selector: "@s", color: "gold"}, {text: ".", color: "gray"}]
