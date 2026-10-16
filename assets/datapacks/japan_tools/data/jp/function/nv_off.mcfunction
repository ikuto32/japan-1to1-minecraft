tag @s remove jp_nv
effect clear @s night_vision
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Night vision OFF", "color": "yellow"}]
tellraw @s [{"text": "夜間視覚 OFF", "color": "yellow"}]
