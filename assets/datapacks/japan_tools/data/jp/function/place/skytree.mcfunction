# 東京スカイツリー / Tokyo Skytree  (35.71005, 139.81071)
tp @s 118164 -1718 31277
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Tokyo Skytree", "color": "gold"}, {"text": "  X 118164  Z 31277  (ground Y -1758)", "color": "gray"}]
tellraw @s [{"text": "東京スカイツリー", "color": "gold"}, {"text": "  X 118164  Z 31277（地表 Y -1758）", "color": "gray"}]
