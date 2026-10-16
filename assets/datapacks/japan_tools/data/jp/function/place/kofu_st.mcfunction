# 甲府駅 / Kofu Station  (35.66708, 138.56939)
tp @s 6259 -1445 36801
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Kofu Station", "color": "gold"}, {"text": "  X 6259  Z 36801  (ground Y -1485)", "color": "gray"}]
tellraw @s [{"text": "甲府駅", "color": "gold"}, {"text": "  X 6259  Z 36801（地表 Y -1485）", "color": "gray"}]
