execute if entity @s[tag=jp_nv] run return run function jp:nv_off
tag @s add jp_nv
effect give @s night_vision infinite 0 true
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Night vision ON", "color": "yellow"}]
tellraw @s [{"text": "夜間視覚 ON", "color": "yellow"}]
