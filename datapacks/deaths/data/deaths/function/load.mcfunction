# Language of the chat messages: lang.de 0 = English (default), 1 = German; shared by all packs
scoreboard objectives add lang.de dummy
scoreboard objectives add deutsch trigger
scoreboard objectives add english trigger
# Deaths since the last check / last damage cause and how long ago (ticks left) / scratch
scoreboard objectives add deaths.n deathCount
scoreboard objectives add deaths.cause dummy
scoreboard objectives add deaths.t dummy
scoreboard objectives add deaths.tmp dummy
