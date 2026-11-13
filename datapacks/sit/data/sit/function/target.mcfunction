# Keep the click target on this block, or put one there (slightly bigger than the lower half, so it
# wins over the block when the crosshair is on the seat surface)
execute positioned ~ ~-0.01 ~ as @e[type=minecraft:interaction, tag=sit.target, distance=..0.05] run return run tag @s add sit.keep
summon minecraft:interaction ~ ~-0.01 ~ {width: 1.02f, height: 0.52f, response: 1b, Tags: ["sit.target", "sit.keep"]}
return 1
