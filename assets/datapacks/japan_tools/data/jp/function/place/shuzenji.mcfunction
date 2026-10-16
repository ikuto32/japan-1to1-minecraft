# 修善寺駅 / Shuzenji Station  (34.97937, 138.95095)
tp @s 41021 -1671 112721
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Shuzenji Station", "color": "gold"}, {"text": "  X 41021  Z 112721  (ground Y -1711)", "color": "gray"}]
tellraw @s [{"text": "修善寺駅", "color": "gold"}, {"text": "  X 41021  Z 112721（地表 Y -1711）", "color": "gray"}]
