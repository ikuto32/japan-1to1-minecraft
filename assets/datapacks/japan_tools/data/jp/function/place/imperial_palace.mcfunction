# 皇居 / Imperial Palace  (35.68291, 139.75038)
tp @s 112763 -1696 34346
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Imperial Palace", "color": "gold"}, {"text": "  X 112763  Z 34346  (ground Y -1736)", "color": "gray"}]
tellraw @s [{"text": "皇居", "color": "gold"}, {"text": "  X 112763  Z 34346（地表 Y -1736）", "color": "gray"}]
