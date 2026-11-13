execute store result score #r deaths.tmp run random value 1..4
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " just wanted to warm up a little.", color: "gray"}]
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " wollte sich nur kurz die Hände wärmen.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " is well done now.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " ist jetzt gut durch.", color: "gray"}]
execute if score #r deaths.tmp matches 3 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " took the campfire a bit too literally.", color: "gray"}]
execute if score #r deaths.tmp matches 3 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " hat das Lagerfeuer zu wörtlich genommen.", color: "gray"}]
execute if score #r deaths.tmp matches 4 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {text: "Anyone smell barbecue? Oh, that's ", color: "gray"}, {selector: "@s", color: "gold"}, {text: ".", color: "gray"}]
execute if score #r deaths.tmp matches 4 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {text: "Riecht hier jemand Grillfleisch? Ach, das ist ", color: "gray"}, {selector: "@s", color: "gold"}, {text: ".", color: "gray"}]
