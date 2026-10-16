execute as @a[tag=jp_chk] at @s unless block ~ -2032 ~ minecraft:bedrock run function jp:not_downloaded
tag @a remove jp_chk
