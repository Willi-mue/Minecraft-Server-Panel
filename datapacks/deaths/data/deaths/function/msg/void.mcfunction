execute store result score #r deaths.tmp run random value 1..3
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " fell into the void. Say hi to nothing.", color: "gray"}]
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " ist ins Nichts gefallen. Grüße ans Nichts.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " is looking for the bottom of the world. There is none.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " sucht jetzt den Boden der Welt. Es gibt keinen.", color: "gray"}]
execute if score #r deaths.tmp matches 3 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " found the end of the world. Literally.", color: "gray"}]
execute if score #r deaths.tmp matches 3 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " hat das Ende der Welt gefunden. Wortwörtlich.", color: "gray"}]
