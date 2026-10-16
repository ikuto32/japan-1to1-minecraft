# 鎌倉大仏 / Great Buddha of Kamakura  (35.31685, 139.53572)
tp @s 93825 -1706 75028
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Great Buddha of Kamakura", "color": "gold"}, {"text": "  X 93825  Z 75028  (ground Y -1746)", "color": "gray"}]
tellraw @s [{"text": "鎌倉大仏", "color": "gold"}, {"text": "  X 93825  Z 75028（地表 Y -1746）", "color": "gray"}]
