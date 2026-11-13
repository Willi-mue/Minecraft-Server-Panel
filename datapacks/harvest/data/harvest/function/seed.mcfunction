# Takes one seed from the player: off hand first, then the hotbar (left to right), then the
# inventory from the top left to the bottom right. 1 = found (nothing taken in creative), 0 = none.
$execute unless items entity @s weapon.offhand minecraft:$(seed) unless items entity @s hotbar.* minecraft:$(seed) unless items entity @s inventory.* minecraft:$(seed) run return 0
execute if entity @s[gamemode=creative] run return 1
$execute if items entity @s weapon.offhand minecraft:$(seed) run return run function harvest:take {slot: "weapon.offhand"}
$execute if items entity @s hotbar.0 minecraft:$(seed) run return run function harvest:take {slot: "hotbar.0"}
$execute if items entity @s hotbar.1 minecraft:$(seed) run return run function harvest:take {slot: "hotbar.1"}
$execute if items entity @s hotbar.2 minecraft:$(seed) run return run function harvest:take {slot: "hotbar.2"}
$execute if items entity @s hotbar.3 minecraft:$(seed) run return run function harvest:take {slot: "hotbar.3"}
$execute if items entity @s hotbar.4 minecraft:$(seed) run return run function harvest:take {slot: "hotbar.4"}
$execute if items entity @s hotbar.5 minecraft:$(seed) run return run function harvest:take {slot: "hotbar.5"}
$execute if items entity @s hotbar.6 minecraft:$(seed) run return run function harvest:take {slot: "hotbar.6"}
$execute if items entity @s hotbar.7 minecraft:$(seed) run return run function harvest:take {slot: "hotbar.7"}
$execute if items entity @s hotbar.8 minecraft:$(seed) run return run function harvest:take {slot: "hotbar.8"}
$execute if items entity @s inventory.0 minecraft:$(seed) run return run function harvest:take {slot: "inventory.0"}
$execute if items entity @s inventory.1 minecraft:$(seed) run return run function harvest:take {slot: "inventory.1"}
$execute if items entity @s inventory.2 minecraft:$(seed) run return run function harvest:take {slot: "inventory.2"}
$execute if items entity @s inventory.3 minecraft:$(seed) run return run function harvest:take {slot: "inventory.3"}
$execute if items entity @s inventory.4 minecraft:$(seed) run return run function harvest:take {slot: "inventory.4"}
$execute if items entity @s inventory.5 minecraft:$(seed) run return run function harvest:take {slot: "inventory.5"}
$execute if items entity @s inventory.6 minecraft:$(seed) run return run function harvest:take {slot: "inventory.6"}
$execute if items entity @s inventory.7 minecraft:$(seed) run return run function harvest:take {slot: "inventory.7"}
$execute if items entity @s inventory.8 minecraft:$(seed) run return run function harvest:take {slot: "inventory.8"}
$execute if items entity @s inventory.9 minecraft:$(seed) run return run function harvest:take {slot: "inventory.9"}
$execute if items entity @s inventory.10 minecraft:$(seed) run return run function harvest:take {slot: "inventory.10"}
$execute if items entity @s inventory.11 minecraft:$(seed) run return run function harvest:take {slot: "inventory.11"}
$execute if items entity @s inventory.12 minecraft:$(seed) run return run function harvest:take {slot: "inventory.12"}
$execute if items entity @s inventory.13 minecraft:$(seed) run return run function harvest:take {slot: "inventory.13"}
$execute if items entity @s inventory.14 minecraft:$(seed) run return run function harvest:take {slot: "inventory.14"}
$execute if items entity @s inventory.15 minecraft:$(seed) run return run function harvest:take {slot: "inventory.15"}
$execute if items entity @s inventory.16 minecraft:$(seed) run return run function harvest:take {slot: "inventory.16"}
$execute if items entity @s inventory.17 minecraft:$(seed) run return run function harvest:take {slot: "inventory.17"}
$execute if items entity @s inventory.18 minecraft:$(seed) run return run function harvest:take {slot: "inventory.18"}
$execute if items entity @s inventory.19 minecraft:$(seed) run return run function harvest:take {slot: "inventory.19"}
$execute if items entity @s inventory.20 minecraft:$(seed) run return run function harvest:take {slot: "inventory.20"}
$execute if items entity @s inventory.21 minecraft:$(seed) run return run function harvest:take {slot: "inventory.21"}
$execute if items entity @s inventory.22 minecraft:$(seed) run return run function harvest:take {slot: "inventory.22"}
$execute if items entity @s inventory.23 minecraft:$(seed) run return run function harvest:take {slot: "inventory.23"}
$execute if items entity @s inventory.24 minecraft:$(seed) run return run function harvest:take {slot: "inventory.24"}
$execute if items entity @s inventory.25 minecraft:$(seed) run return run function harvest:take {slot: "inventory.25"}
$execute if items entity @s inventory.26 minecraft:$(seed) run return run function harvest:take {slot: "inventory.26"}
return 0
