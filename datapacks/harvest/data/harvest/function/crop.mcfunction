# Only ripe crops (returns 1 when harvested, 0 = unripe, keep looking)
execute if block ~ ~ ~ minecraft:wheat[age=7] run return run function harvest:pick {block: "wheat", seed: "wheat_seeds", sound: "crop"}
execute if block ~ ~ ~ minecraft:carrots[age=7] run return run function harvest:pick {block: "carrots", seed: "carrot", sound: "crop"}
execute if block ~ ~ ~ minecraft:potatoes[age=7] run return run function harvest:pick {block: "potatoes", seed: "potato", sound: "crop"}
execute if block ~ ~ ~ minecraft:beetroots[age=3] run return run function harvest:pick {block: "beetroots", seed: "beetroot_seeds", sound: "crop"}
execute if block ~ ~ ~ minecraft:nether_wart[age=3] run return run function harvest:pick {block: "nether_wart", seed: "nether_wart", sound: "nether_wart"}
return 0
