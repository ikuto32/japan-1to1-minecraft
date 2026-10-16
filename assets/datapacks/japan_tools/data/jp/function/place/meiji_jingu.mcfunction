# 明治神宮 / Meiji Jingu  (35.67592, 139.7005)
tp @s 108275 -1685 35174
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Meiji Jingu", "color": "gold"}, {"text": "  X 108275  Z 35174  (ground Y -1725)", "color": "gray"}]
tellraw @s [{"text": "明治神宮", "color": "gold"}, {"text": "  X 108275  Z 35174（地表 Y -1725）", "color": "gray"}]
