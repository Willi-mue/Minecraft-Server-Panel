# /trigger deutsch | english (the same in every pack with chat messages)
scoreboard players set @s deutsch 0
scoreboard players set @s english 0
$scoreboard players set @s lang.de $(de)
tellraw @s[scores={lang.de=0}] {text: "Language: English. Type /trigger deutsch for German.", color: "gray"}
tellraw @s[scores={lang.de=1}] {text: "Sprache: Deutsch. Mit /trigger english zurück auf Englisch.", color: "gray"}
