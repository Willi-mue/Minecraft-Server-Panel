execute store result score #r deaths.tmp run random value 1..2
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " tried the wrong potion.", color: "gray"}]
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " hat den falschen Trank probiert.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {text: "Witchcraft! ", color: "gray"}, {selector: "@s", color: "gold"}, {text: " got hit.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {text: "Hexerei! ", color: "gray"}, {selector: "@s", color: "gold"}, {text: " hat es erwischt.", color: "gray"}]
