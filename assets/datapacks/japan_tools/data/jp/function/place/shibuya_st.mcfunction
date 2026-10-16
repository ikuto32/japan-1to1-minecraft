# 渋谷駅 / Shibuya Station  (35.65808, 139.70176)
tp @s 108412 -1705 37145
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Shibuya Station", "color": "gold"}, {"text": "  X 108412  Z 37145  (ground Y -1745)", "color": "gray"}]
tellraw @s [{"text": "渋谷駅", "color": "gold"}, {"text": "  X 108412  Z 37145（地表 Y -1745）", "color": "gray"}]
