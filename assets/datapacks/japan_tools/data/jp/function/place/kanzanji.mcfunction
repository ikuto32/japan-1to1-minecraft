# 浜名湖 舘山寺 / Lake Hamana (Kanzanji)  (34.76822, 137.61309)
tp @s -80884 -1707 135791
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Lake Hamana (Kanzanji)", "color": "gold"}, {"text": "  X -80884  Z 135791  (ground Y -1747)", "color": "gray"}]
tellraw @s [{"text": "浜名湖 舘山寺", "color": "gold"}, {"text": "  X -80884  Z 135791（地表 Y -1747）", "color": "gray"}]
