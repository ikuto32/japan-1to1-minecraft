execute if entity @s[tag=jp_hud] run return run function jp:hud_off
tag @s add jp_hud
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Position display ON (elevation, latitude/longitude to about 30 m)", "color": "yellow"}]
tellraw @s [{"text": "現在地表示 ON（標高と緯度経度。緯度経度は約30 m の精度）", "color": "yellow"}]
