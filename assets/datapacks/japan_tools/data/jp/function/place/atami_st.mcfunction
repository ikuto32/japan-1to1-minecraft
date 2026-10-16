# 熱海駅 / Atami Station  (35.10385, 139.07812)
tp @s 52509 -1648 98904
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Atami Station", "color": "gold"}, {"text": "  X 52509  Z 98904  (ground Y -1688)", "color": "gray"}]
tellraw @s [{"text": "熱海駅", "color": "gold"}, {"text": "  X 52509  Z 98904（地表 Y -1688）", "color": "gray"}]
