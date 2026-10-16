# 富士山 富士宮口五合目 / Mt. Fuji Fujinomiya 5th Station  (35.33685, 138.733)
tp @s 21103 657 73280
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Mt. Fuji Fujinomiya 5th Station", "color": "gold"}, {"text": "  X 21103  Z 73280  (ground Y 617)", "color": "gray"}]
tellraw @s [{"text": "富士山 富士宮口五合目", "color": "gold"}, {"text": "  X 21103  Z 73280（地表 Y 617）", "color": "gray"}]
