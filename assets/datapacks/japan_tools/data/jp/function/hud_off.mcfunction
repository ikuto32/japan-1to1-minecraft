tag @s remove jp_hud
title @s actionbar ""
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Position display OFF", "color": "yellow"}]
tellraw @s [{"text": "現在地表示 OFF", "color": "yellow"}]
