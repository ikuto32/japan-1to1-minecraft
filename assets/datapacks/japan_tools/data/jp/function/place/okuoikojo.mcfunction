# 奥大井湖上駅 / Okuoikojo Station  (35.16803, 138.18105)
tp @s -28947 -1230 91917
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Okuoikojo Station", "color": "gold"}, {"text": "  X -28947  Z 91917  (ground Y -1270)", "color": "gray"}]
tellraw @s [{"text": "奥大井湖上駅", "color": "gold"}, {"text": "  X -28947  Z 91917（地表 Y -1270）", "color": "gray"}]
