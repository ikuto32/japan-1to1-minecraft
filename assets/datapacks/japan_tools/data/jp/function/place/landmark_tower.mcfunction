# 横浜ランドマークタワー / Yokohama Landmark Tower  (35.45458, 139.63142)
tp @s 102322 -1716 59711
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Yokohama Landmark Tower", "color": "gold"}, {"text": "  X 102322  Z 59711  (ground Y -1756)", "color": "gray"}]
tellraw @s [{"text": "横浜ランドマークタワー", "color": "gold"}, {"text": "  X 102322  Z 59711（地表 Y -1756）", "color": "gray"}]
