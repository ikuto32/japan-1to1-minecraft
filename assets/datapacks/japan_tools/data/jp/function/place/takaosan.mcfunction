# 高尾山 / Mt. Takao  (35.625, 139.2439)
tp @s 67136 -1128 41204
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Mt. Takao", "color": "gold"}, {"text": "  X 67136  Z 41204  (ground Y -1168)", "color": "gray"}]
tellraw @s [{"text": "高尾山", "color": "gold"}, {"text": "  X 67136  Z 41204（地表 Y -1168）", "color": "gray"}]
