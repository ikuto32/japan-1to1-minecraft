# 浅草 雷門 / Asakusa Kaminarimon  (35.71112, 139.79638)
tp @s 116871 -1716 31175
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Asakusa Kaminarimon", "color": "gold"}, {"text": "  X 116871  Z 31175  (ground Y -1756)", "color": "gray"}]
tellraw @s [{"text": "浅草 雷門", "color": "gold"}, {"text": "  X 116871  Z 31175（地表 Y -1756）", "color": "gray"}]
