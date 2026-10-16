# 横浜駅 / Yokohama Station  (35.46621, 139.6232)
tp @s 101564 -1717 58434
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Yokohama Station", "color": "gold"}, {"text": "  X 101564  Z 58434  (ground Y -1757)", "color": "gray"}]
tellraw @s [{"text": "横浜駅", "color": "gold"}, {"text": "  X 101564  Z 58434（地表 Y -1757）", "color": "gray"}]
