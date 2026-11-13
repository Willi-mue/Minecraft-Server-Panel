execute unless data entity @s LastDeathLocation run return 0
data modify storage deaths:tmp a set value {en: "", de: ""}
data modify storage deaths:tmp a.x set from entity @s LastDeathLocation.pos[0]
data modify storage deaths:tmp a.y set from entity @s LastDeathLocation.pos[1]
data modify storage deaths:tmp a.z set from entity @s LastDeathLocation.pos[2]
data modify storage deaths:tmp a.dim set from entity @s LastDeathLocation.dimension
data modify storage deaths:tmp a.en set from entity @s LastDeathLocation.dimension
data modify storage deaths:tmp a.de set from entity @s LastDeathLocation.dimension
execute if data entity @s LastDeathLocation{dimension: "minecraft:overworld"} run data modify storage deaths:tmp a merge value {en: "Overworld", de: "Oberwelt"}
execute if data entity @s LastDeathLocation{dimension: "minecraft:the_nether"} run data modify storage deaths:tmp a merge value {en: "Nether", de: "Nether"}
execute if data entity @s LastDeathLocation{dimension: "minecraft:the_end"} run data modify storage deaths:tmp a merge value {en: "End", de: "End"}
function deaths:tell with storage deaths:tmp a
