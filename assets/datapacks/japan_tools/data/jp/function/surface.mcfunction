execute store success score #ok jp_t run spreadplayers ~ ~ 0 1 false @s
execute if score #ok jp_t matches 0 run return run function jp:missing
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Moved down onto the top block below you", "color": "yellow"}]
tellraw @s [{"text": "真下の一番上のブロックに降りました", "color": "yellow"}]
