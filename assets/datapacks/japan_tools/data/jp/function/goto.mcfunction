# usage: /function jp:goto {x:-10094,z:113663}  (lands on the top block at X, Z)
$execute store success score #ok jp_t run spreadplayers $(x) $(z) 0 1 false @s
execute if score #ok jp_t matches 0 run return run function jp:missing
$execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Moved to X $(x) Z $(z)", "color": "yellow"}]
$tellraw @s [{"text": "X $(x) Z $(z) に移動しました", "color": "yellow"}]
