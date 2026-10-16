# 石廊崎 / Cape Irozaki  (34.60195, 138.84605)
tp @s 31623 -1704 154471
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Cape Irozaki", "color": "gold"}, {"text": "  X 31623  Z 154471  (ground Y -1744)", "color": "gray"}]
tellraw @s [{"text": "石廊崎", "color": "gold"}, {"text": "  X 31623  Z 154471（地表 Y -1744）", "color": "gray"}]
