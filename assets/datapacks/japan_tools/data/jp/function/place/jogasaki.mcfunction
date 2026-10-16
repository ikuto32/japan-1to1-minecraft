# 城ヶ崎海岸 / Jogasaki Coast  (34.89042, 139.13858)
tp @s 58151 -1695 122459
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Jogasaki Coast", "color": "gold"}, {"text": "  X 58151  Z 122459  (ground Y -1735)", "color": "gray"}]
tellraw @s [{"text": "城ヶ崎海岸", "color": "gold"}, {"text": "  X 58151  Z 122459（地表 Y -1735）", "color": "gray"}]
