# 東海道新幹線 富士川橋梁 / Tokaido Shinkansen Fujikawa Bridge  (35.13813, 138.630853)
tp @s 11880 -1712 95260
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Tokaido Shinkansen Fujikawa Bridge", "color": "gold"}, {"text": "  X 11880  Z 95260  (ground Y -1752)", "color": "gray"}]
tellraw @s [{"text": "東海道新幹線 富士川橋梁", "color": "gold"}, {"text": "  X 11880  Z 95260（地表 Y -1752）", "color": "gray"}]
