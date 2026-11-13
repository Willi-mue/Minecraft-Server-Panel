# As a dropped item: into the new grave
data modify entity @n[type=minecraft:marker, tag=graves.new] data.items append from entity @s Item
kill @s
