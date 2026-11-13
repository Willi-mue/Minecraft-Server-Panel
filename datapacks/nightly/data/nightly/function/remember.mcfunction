# As one of the best players: a data pack cannot turn a selector into a string directly, but the
# set_name item modifier resolves it into a plain text name on a temporary item
summon minecraft:item_display ~ ~ ~ {Tags: ["nightly.name"], item: {id: "minecraft:paper", count: 1}}
item modify entity @n[type=minecraft:item_display, tag=nightly.name] contents nightly:names
$data modify storage nightly:last r.$(key).name set from entity @n[type=minecraft:item_display, tag=nightly.name] item.components."minecraft:custom_name"
kill @n[type=minecraft:item_display, tag=nightly.name]
