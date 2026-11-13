# Drops graves:tmp items one by one here (at the player: picked up at once)
execute unless data storage graves:tmp items[0] run return 0
data modify storage graves:tmp one.item set from storage graves:tmp items[0]
function graves:spawn_one with storage graves:tmp one
data remove storage graves:tmp items[0]
function graves:spawn_items
