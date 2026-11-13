# Language of the chat messages: lang.de 0 = English (default), 1 = German; shared by all packs
scoreboard objectives add lang.de dummy
scoreboard objectives add deutsch trigger
scoreboard objectives add english trigger
# Counted from dusk to dawn (only players who were online)
scoreboard objectives add nightly.kills minecraft.custom:minecraft.mob_kills
scoreboard objectives add nightly.deaths deathCount
scoreboard objectives add nightly.walk minecraft.custom:minecraft.walk_one_cm
scoreboard objectives add nightly.sprint minecraft.custom:minecraft.sprint_one_cm
# The player's own values of the last night (for /trigger nachtbilanz)
scoreboard objectives add nightly.lk dummy
scoreboard objectives add nightly.ld dummy
scoreboard objectives add nightly.lw dummy
# /trigger nachtbilanz (or /trigger lastnight) shows the last report again (any player, no op needed)
scoreboard objectives add nachtbilanz trigger
scoreboard objectives add lastnight trigger
# #night = 1 while a night is being counted; maxima and scratch values
scoreboard objectives add nightly.tmp dummy
scoreboard players set #100 nightly.tmp 100
