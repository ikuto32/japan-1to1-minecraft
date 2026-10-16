# 昇仙峡 / Shosenkyo Gorge  (35.7459, 138.56749)
tp @s 6082 -1099 28088
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Shosenkyo Gorge", "color": "gold"}, {"text": "  X 6082  Z 28088  (ground Y -1139)", "color": "gray"}]
tellraw @s [{"text": "昇仙峡", "color": "gold"}, {"text": "  X 6082  Z 28088（地表 Y -1139）", "color": "gray"}]
