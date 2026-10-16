# 牧之原台地の茶畑 / Makinohara tea plantations  (34.795, 138.155)
tp @s -31453 -1630 133136
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Makinohara tea plantations", "color": "gold"}, {"text": "  X -31453  Z 133136  (ground Y -1670)", "color": "gray"}]
tellraw @s [{"text": "牧之原台地の茶畑", "color": "gold"}, {"text": "  X -31453  Z 133136（地表 Y -1670）", "color": "gray"}]
