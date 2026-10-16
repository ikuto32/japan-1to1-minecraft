execute as @a[tag=!jp_seen] run function jp:welcome
scoreboard players add #tick jp_t 1
execute if score #tick jp_t matches 10.. run function jp:tick10
