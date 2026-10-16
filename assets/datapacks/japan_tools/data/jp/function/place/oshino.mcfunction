# 忍野八海 / Oshino Hakkai  (35.46024, 138.8327)
tp @s 30087 -790 59616
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Oshino Hakkai", "color": "gold"}, {"text": "  X 30087  Z 59616  (ground Y -830)", "color": "gray"}]
tellraw @s [{"text": "忍野八海", "color": "gold"}, {"text": "  X 30087  Z 59616（地表 Y -830）", "color": "gray"}]
