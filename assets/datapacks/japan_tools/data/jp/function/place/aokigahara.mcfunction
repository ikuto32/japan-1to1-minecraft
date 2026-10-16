# 青木ヶ原樹海（富岳風穴） / Aokigahara forest (Fugaku Wind Cave)  (35.47912, 138.65944)
tp @s 14415 -729 57568
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Aokigahara forest (Fugaku Wind Cave)", "color": "gold"}, {"text": "  X 14415  Z 57568  (ground Y -769)", "color": "gray"}]
tellraw @s [{"text": "青木ヶ原樹海（富岳風穴）", "color": "gold"}, {"text": "  X 14415  Z 57568（地表 Y -769）", "color": "gray"}]
