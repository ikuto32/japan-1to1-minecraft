execute if entity @s[tag=jp_step] run return run function jp:step_off
tag @s add jp_step
attribute @s minecraft:step_height base set 1.05
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Auto step-up (1 block) ON", "color": "yellow"}]
tellraw @s [{"text": "1 段の段差を自動で上る ON", "color": "yellow"}]
