# 羽田空港 / Haneda Airport  (35.5533, 139.7811)
tp @s 115718 -1716 48637
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Haneda Airport", "color": "gold"}, {"text": "  X 115718  Z 48637  (ground Y -1756)", "color": "gray"}]
tellraw @s [{"text": "羽田空港", "color": "gold"}, {"text": "  X 115718  Z 48637（地表 Y -1756）", "color": "gray"}]
