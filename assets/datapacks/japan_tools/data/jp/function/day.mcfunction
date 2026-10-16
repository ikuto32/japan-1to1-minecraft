gamerule minecraft:advance_time false
gamerule minecraft:advance_weather false
time set noon
weather clear
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Fixed to midday and clear weather (undo: /function jp:time_normal)", "color": "yellow"}]
tellraw @s [{"text": "昼・晴れに固定しました（元に戻す: /function jp:time_normal）", "color": "yellow"}]
