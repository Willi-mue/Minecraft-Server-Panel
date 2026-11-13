# Cooldown between two harvests while the right mouse button is held (ticks)
scoreboard objectives add harvest.cd dummy
# Ray steps left / scratch values
scoreboard objectives add harvest.tmp dummy
# 1 = looking at a crop with a hoe
scoreboard objectives add harvest.aim dummy
data remove storage harvest:state off
