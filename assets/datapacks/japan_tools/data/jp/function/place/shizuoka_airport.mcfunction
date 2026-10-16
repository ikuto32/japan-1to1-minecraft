# 富士山静岡空港 / Mt. Fuji Shizuoka Airport  (34.79377, 138.19105)
tp @s -28167 -1591 133283
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Mt. Fuji Shizuoka Airport", "color": "gold"}, {"text": "  X -28167  Z 133283  (ground Y -1631)", "color": "gray"}]
tellraw @s [{"text": "富士山静岡空港", "color": "gold"}, {"text": "  X -28167  Z 133283（地表 Y -1631）", "color": "gray"}]
