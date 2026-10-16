# 富士急ハイランド / Fuji-Q Highland  (35.487, 138.78005)
tp @s 25317 -881 56673
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Fuji-Q Highland", "color": "gold"}, {"text": "  X 25317  Z 56673  (ground Y -921)", "color": "gray"}]
tellraw @s [{"text": "富士急ハイランド", "color": "gold"}, {"text": "  X 25317  Z 56673（地表 Y -921）", "color": "gray"}]
