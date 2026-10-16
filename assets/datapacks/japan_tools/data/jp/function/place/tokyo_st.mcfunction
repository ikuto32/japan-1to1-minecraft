# 東京駅 / Tokyo Station  (35.68114, 139.76746)
tp @s 114306 -1716 34523
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Tokyo Station", "color": "gold"}, {"text": "  X 114306  Z 34523  (ground Y -1756)", "color": "gray"}]
tellraw @s [{"text": "東京駅", "color": "gold"}, {"text": "  X 114306  Z 34523（地表 Y -1756）", "color": "gray"}]
