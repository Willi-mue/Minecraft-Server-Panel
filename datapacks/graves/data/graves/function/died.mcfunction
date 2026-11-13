# As the player who just died (the items dropped this tick lie around the death point)
scoreboard players set @s graves.d 0
execute unless data entity @s LastDeathLocation run return 0
data modify storage graves:tmp a set value {}
data modify storage graves:tmp a.x set from entity @s LastDeathLocation.pos[0]
data modify storage graves:tmp a.y set from entity @s LastDeathLocation.pos[1]
data modify storage graves:tmp a.z set from entity @s LastDeathLocation.pos[2]
data modify storage graves:tmp a.dim set from entity @s LastDeathLocation.dimension
function graves:at with storage graves:tmp a
