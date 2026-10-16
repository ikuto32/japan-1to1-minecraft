# 箱根関所（芦ノ湖） / Hakone Checkpoint (Lake Ashi)  (35.19239, 139.02639)
tp @s 47759 -988 89145
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Hakone Checkpoint (Lake Ashi)", "color": "gold"}, {"text": "  X 47759  Z 89145  (ground Y -1028)", "color": "gray"}]
tellraw @s [{"text": "箱根関所（芦ノ湖）", "color": "gold"}, {"text": "  X 47759  Z 89145（地表 Y -1028）", "color": "gray"}]
