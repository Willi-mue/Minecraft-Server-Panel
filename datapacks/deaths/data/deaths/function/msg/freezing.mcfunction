execute store result score #r deaths.tmp run random value 1..3
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " is a popsicle now.", color: "gray"}]
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " ist jetzt ein Eis am Stiel.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " should have worn the leather boots.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " hätte doch die Lederstiefel anziehen sollen.", color: "gray"}]
execute if score #r deaths.tmp matches 3 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {text: "Brrr. ", color: "gray"}, {selector: "@s", color: "gold"}, {text: " stays cool forever.", color: "gray"}]
execute if score #r deaths.tmp matches 3 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {text: "Brrr. ", color: "gray"}, {selector: "@s", color: "gold"}, {text: " bleibt jetzt für immer cool.", color: "gray"}]
