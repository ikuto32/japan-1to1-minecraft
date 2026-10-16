# 日本平 / Nihondaira  (34.97673, 138.46662)
tp @s -3037 -1446 113105
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Nihondaira", "color": "gold"}, {"text": "  X -3037  Z 113105  (ground Y -1486)", "color": "gray"}]
tellraw @s [{"text": "日本平", "color": "gold"}, {"text": "  X -3037  Z 113105（地表 Y -1486）", "color": "gray"}]
