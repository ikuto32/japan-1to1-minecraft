execute store result score #fd jp_t run gamerule minecraft:fall_damage
execute if score #fd jp_t matches 1 run gamerule minecraft:fall_damage false
execute if score #fd jp_t matches 0 run gamerule minecraft:fall_damage true
execute if score #fd jp_t matches 1 run return run function jp:fall_msg_off
function jp:fall_msg_on
