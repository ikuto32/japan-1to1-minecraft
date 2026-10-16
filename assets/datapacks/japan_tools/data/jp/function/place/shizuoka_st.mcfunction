# 静岡駅 / Shizuoka Station  (34.97169, 138.38932)
tp @s -10069 -1705 113657
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Shizuoka Station", "color": "gold"}, {"text": "  X -10069  Z 113657  (ground Y -1745)", "color": "gray"}]
tellraw @s [{"text": "静岡駅", "color": "gold"}, {"text": "  X -10069  Z 113657（地表 Y -1745）", "color": "gray"}]
