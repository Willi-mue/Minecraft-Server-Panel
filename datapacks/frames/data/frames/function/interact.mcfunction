# Runs as the player right after clicking a frame (any frame, any item)
advancement revoke @s only frames:interact
execute unless predicate frames:sneaking run return 0
execute as @n[type=#frames:frames, tag=frames.aim, distance=..8] run function frames:toggle
