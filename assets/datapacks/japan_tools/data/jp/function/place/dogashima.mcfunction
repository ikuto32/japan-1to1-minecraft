# 堂ヶ島 / Dogashima  (34.78241, 138.76742)
tp @s 24384 -1715 134549
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Dogashima", "color": "gold"}, {"text": "  X 24384  Z 134549  (ground Y -1755)", "color": "gray"}]
tellraw @s [{"text": "堂ヶ島", "color": "gold"}, {"text": "  X 24384  Z 134549（地表 Y -1755）", "color": "gray"}]
