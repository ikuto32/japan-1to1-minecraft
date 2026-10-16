effect clear @s slow_falling
tp @s 20583 2016 70641
execute if score @s jp_lang matches 1 run return run tellraw @s [{"text": "That area does not seem to be downloaded (add its tile on the tile selection page). Sent you back to spawn (summit of Mt. Fuji).", "color": "red"}]
tellraw @s [{"text": "その場所のタイルはダウンロードされていないようです（タイル選択ページで追加できます）。スポーン地点（富士山頂）に戻しました", "color": "red"}]
