# 三島スカイウォーク / Mishima Skywalk  (35.15188, 138.98063)
tp @s 43629 -1339 93643
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Mishima Skywalk", "color": "gold"}, {"text": "  X 43629  Z 93643  (ground Y -1379)", "color": "gray"}]
tellraw @s [{"text": "三島スカイウォーク", "color": "gold"}, {"text": "  X 43629  Z 93643（地表 Y -1379）", "color": "gray"}]
