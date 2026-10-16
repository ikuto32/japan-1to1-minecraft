# 北岳 / Mt. Kita  (35.674321, 138.238808)
tp @s -23558 1473 35972
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Mt. Kita", "color": "gold"}, {"text": "  X -23558  Z 35972  (ground Y 1433)", "color": "gray"}]
tellraw @s [{"text": "北岳", "color": "gold"}, {"text": "  X -23558  Z 35972（地表 Y 1433）", "color": "gray"}]
