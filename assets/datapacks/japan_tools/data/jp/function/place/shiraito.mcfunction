# 白糸の滝 / Shiraito Falls  (35.31299, 138.58722)
tp @s 7902 -1261 75938
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Shiraito Falls", "color": "gold"}, {"text": "  X 7902  Z 75938  (ground Y -1301)", "color": "gray"}]
tellraw @s [{"text": "白糸の滝", "color": "gold"}, {"text": "  X 7902  Z 75938（地表 Y -1301）", "color": "gray"}]
