# 三保の松原 / Miho no Matsubara  (34.99474, 138.5238)
tp @s 2165 -1712 111115
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Miho no Matsubara", "color": "gold"}, {"text": "  X 2165  Z 111115  (ground Y -1752)", "color": "gray"}]
tellraw @s [{"text": "三保の松原", "color": "gold"}, {"text": "  X 2165  Z 111115（地表 Y -1752）", "color": "gray"}]
