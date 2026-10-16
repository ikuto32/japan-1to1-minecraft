# 箱根 大涌谷 / Hakone Owakudani  (35.24649, 139.02408)
tp @s 47518 -832 83167
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Hakone Owakudani", "color": "gold"}, {"text": "  X 47518  Z 83167  (ground Y -872)", "color": "gray"}]
tellraw @s [{"text": "箱根 大涌谷", "color": "gold"}, {"text": "  X 47518  Z 83167（地表 Y -872）", "color": "gray"}]
