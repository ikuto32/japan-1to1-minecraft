gamerule minecraft:advance_time true
gamerule minecraft:advance_weather true
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Time and weather run normally again", "color": "yellow"}]
tellraw @s [{"text": "時間と天気が進むようにしました", "color": "yellow"}]
