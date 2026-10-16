# 河口湖駅 / Kawaguchiko Station  (35.49803, 138.76898)
tp @s 24313 -863 55456
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Kawaguchiko Station", "color": "gold"}, {"text": "  X 24313  Z 55456  (ground Y -903)", "color": "gray"}]
tellraw @s [{"text": "河口湖駅", "color": "gold"}, {"text": "  X 24313  Z 55456（地表 Y -903）", "color": "gray"}]
