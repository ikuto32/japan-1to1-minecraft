# 富士スバルライン五合目 / Mt. Fuji Subaru Line 5th Station  (35.3951, 138.7327)
tp @s 21060 569 66841
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Mt. Fuji Subaru Line 5th Station", "color": "gold"}, {"text": "  X 21060  Z 66841  (ground Y 529)", "color": "gray"}]
tellraw @s [{"text": "富士スバルライン五合目", "color": "gold"}, {"text": "  X 21060  Z 66841（地表 Y 529）", "color": "gray"}]
