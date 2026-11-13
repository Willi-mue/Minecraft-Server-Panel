execute store result score #r deaths.tmp run random value 1..4
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " took a lava bath. A bit too warm.", color: "gray"}]
execute if score #r deaths.tmp matches 1 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " hat ein Bad in der Lava genommen. Etwas zu warm.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " just wanted to check the temperature.", color: "gray"}]
execute if score #r deaths.tmp matches 2 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " wollte nur mal die Temperatur prüfen.", color: "gray"}]
execute if score #r deaths.tmp matches 3 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {text: "The lava says thanks for the gear, ", color: "gray"}, {selector: "@s", color: "gold"}, {text: ".", color: "gray"}]
execute if score #r deaths.tmp matches 3 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {text: "Die Lava bedankt sich für die Ausrüstung, ", color: "gray"}, {selector: "@s", color: "gold"}, {text: ".", color: "gray"}]
execute if score #r deaths.tmp matches 4 run tellraw @a[scores={lang.de=0}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " is part of the underworld now.", color: "gray"}]
execute if score #r deaths.tmp matches 4 run tellraw @a[scores={lang.de=1}] [{text: "☠ ", color: "dark_gray"}, {selector: "@s", color: "gold"}, {text: " ist jetzt ein Teil der Unterwelt.", color: "gray"}]
