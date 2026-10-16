# 東京タワー / Tokyo Tower  (35.658581, 139.745433)
tp @s 112351 -1701 37041
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Tokyo Tower", "color": "gold"}, {"text": "  X 112351  Z 37041  (ground Y -1741)", "color": "gray"}]
tellraw @s [{"text": "東京タワー", "color": "gold"}, {"text": "  X 112351  Z 37041（地表 Y -1741）", "color": "gray"}]
