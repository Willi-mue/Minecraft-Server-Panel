execute store result score #r deaths.tmp run random value 1..2
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " was too loud. The Warden doesn't like that.", color: "gray"}]
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " war zu laut. Der Warden mag das nicht.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {text: "Shh, ", color: "gray"}, {selector: "@s", color: "gold"}, {text: "! Too late.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {text: "Psst, ", color: "gray"}, {selector: "@s", color: "gold"}, {text: "! Zu spät.", color: "gray"}]
