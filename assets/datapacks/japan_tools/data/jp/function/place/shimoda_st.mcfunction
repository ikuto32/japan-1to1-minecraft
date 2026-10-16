# 伊豆急下田駅 / Izukyu-Shimoda Station  (34.67945, 138.94432)
tp @s 40565 -1717 145870
effect give @s slow_falling 30 0 true
tag @s add jp_chk
schedule function jp:check_ground 40t replace
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Izukyu-Shimoda Station", "color": "gold"}, {"text": "  X 40565  Z 145870  (ground Y -1757)", "color": "gray"}]
tellraw @s [{"text": "伊豆急下田駅", "color": "gold"}, {"text": "  X 40565  Z 145870（地表 Y -1757）", "color": "gray"}]
