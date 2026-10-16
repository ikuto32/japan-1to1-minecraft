tag @s remove jp_step
attribute @s minecraft:step_height base reset
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Auto step-up OFF", "color": "yellow"}]
tellraw @s [{"text": "自動で上る OFF", "color": "yellow"}]
