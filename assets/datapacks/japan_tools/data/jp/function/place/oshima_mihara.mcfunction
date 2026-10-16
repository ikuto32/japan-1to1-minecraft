# 伊豆大島 三原山 / Izu Oshima, Mt. Mihara  (34.724475, 139.394436)
tp @s 81614 -962 140619
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Izu Oshima, Mt. Mihara", "color": "gold"}, {"text": "  X 81614  Z 140619  (ground Y -1002)", "color": "gray"}]
tellraw @s [{"text": "伊豆大島 三原山", "color": "gold"}, {"text": "  X 81614  Z 140619（地表 Y -1002）", "color": "gray"}]
