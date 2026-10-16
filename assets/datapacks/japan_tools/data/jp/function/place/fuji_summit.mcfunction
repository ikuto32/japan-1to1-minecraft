# 富士山頂（剣ヶ峰） / Mt. Fuji summit (Kengamine)  (35.360681, 138.727351)
tp @s 20585 2016 70647
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Mt. Fuji summit (Kengamine)", "color": "gold"}, {"text": "  X 20585  Z 70647  (ground Y 2014)", "color": "gray"}]
tellraw @s [{"text": "富士山頂（剣ヶ峰）", "color": "gold"}, {"text": "  X 20585  Z 70647（地表 Y 2014）", "color": "gray"}]
