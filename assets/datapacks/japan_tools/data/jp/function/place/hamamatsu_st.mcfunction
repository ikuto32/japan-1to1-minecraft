# 浜松駅 / Hamamatsu Station  (34.70401, 137.73481)
tp @s -69838 -1716 142979
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Hamamatsu Station", "color": "gold"}, {"text": "  X -69838  Z 142979  (ground Y -1756)", "color": "gray"}]
tellraw @s [{"text": "浜松駅", "color": "gold"}, {"text": "  X -69838  Z 142979（地表 Y -1756）", "color": "gray"}]
