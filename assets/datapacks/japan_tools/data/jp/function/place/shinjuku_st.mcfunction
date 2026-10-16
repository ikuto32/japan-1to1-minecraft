# 新宿駅 / Shinjuku Station  (35.6906, 139.7003)
tp @s 108237 -1683 33552
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Shinjuku Station", "color": "gold"}, {"text": "  X 108237  Z 33552  (ground Y -1723)", "color": "gray"}]
tellraw @s [{"text": "新宿駅", "color": "gold"}, {"text": "  X 108237  Z 33552（地表 Y -1723）", "color": "gray"}]
