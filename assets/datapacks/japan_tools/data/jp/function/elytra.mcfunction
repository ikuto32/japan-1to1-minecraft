give @s elytra[unbreakable={}] 1
give @s firework_rocket[fireworks={flight_duration:3}] 64
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "Gave an elytra and rockets: wear it, jump while falling to glide, use rockets to boost", "color": "yellow"}]
tellraw @s [{"text": "エリトラとロケット花火を渡しました。胸に装備し、落下中にジャンプで滑空、花火で加速します", "color": "yellow"}]
