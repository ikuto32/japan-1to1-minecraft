# 江の島 / Enoshima  (35.29906, 139.47895)
tp @s 88702 -1670 77046
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Enoshima", "color": "gold"}, {"text": "  X 88702  Z 77046  (ground Y -1710)", "color": "gray"}]
tellraw @s [{"text": "江の島", "color": "gold"}, {"text": "  X 88702  Z 77046（地表 Y -1710）", "color": "gray"}]
