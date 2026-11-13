# As the frame
tag @s add frames.aim
tag @s remove frames.was_empty
execute unless data entity @s Item run tag @s add frames.was_empty
execute store result score @s frames.rot run data get entity @s ItemRotation
return 1
