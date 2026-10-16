#!/usr/bin/env bash
# 日本 1:1 Minecraft ワールド — 選んだタイル（10,240 ブロック四方）だけをダウンロードして展開します。
# Japan 1:1 Minecraft world — downloads and unpacks only the tiles (10,240 blocks square) you choose.
#
#   bash download.sh x-20480_z112640 x-10240_z112640
#   bash download.sh -f tiles.txt -d ~/minecraft/saves
#   bash download.sh all
#
# Options: -d DIR  saves folder (default: the Minecraft saves folder of this OS)
#          -f FILE tile IDs, one per line     -k keep the downloaded zips     -y don't ask
# Needs bash 3.2+, curl, unzip and sha256sum or shasum. Safe to re-run: only missing tiles are fetched.
# JAPAN1M_BASE_URL=... overrides the download location (mirrors, testing).
set -euo pipefail

REPO="${JAPAN1M_REPO:-__REPO__}"
TAG="${JAPAN1M_TAG:-__TAG__}"
WORLD="__WORLD__"
BASE_URL="${JAPAN1M_BASE_URL:-https://github.com/$REPO/releases/download/$TAG}"

case "$(uname -s)" in
  Darwin) DEST="$HOME/Library/Application Support/minecraft/saves" ;;
  *) DEST="$HOME/.minecraft/saves" ;;
esac
KEEP=0
YES=0
TILES=()
while [ $# -gt 0 ]; do
  case "$1" in
    -d) DEST="$2"; shift 2 ;;
    -f) while IFS= read -r l || [ -n "$l" ]; do for t in ${l//[,;]/ }; do TILES+=("$t"); done; done < "$2"; shift 2 ;;
    -k) KEEP=1; shift ;;
    -y) YES=1; shift ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    *) for t in ${1//[,;]/ }; do TILES+=("$t"); done; shift ;;
  esac
done
[ ${#TILES[@]} -gt 0 ] || { echo "タイルを指定してください / specify tiles (see -h)" >&2; exit 2; }
for c in curl unzip; do command -v "$c" >/dev/null || { echo "$c が必要です / $c is required" >&2; exit 2; }; done
if command -v sha256sum >/dev/null; then SHA="sha256sum"; else SHA="shasum -a 256"; fi

WORLD_DIR="$DEST/$WORLD"
CACHE="$DEST/_${WORLD}_downloads"
MARKS="$WORLD_DIR/.downloaded"
mkdir -p "$WORLD_DIR" "$CACHE" "$MARKS"

echo "manifest: $BASE_URL/tiles.tsv"
curl -fsSL --retry 5 -o "$CACHE/tiles.tsv" "$BASE_URL/tiles.tsv"

lookup() { awk -F'\t' -v id="$1" 'NR > 1 && $1 == id { print $2 "\t" $3 "\t" $4 }' "$CACHE/tiles.tsv"; }
if printf '%s\n' "${TILES[@]}" | grep -qx all; then
  TILES=()
  while IFS= read -r t; do TILES+=("$t"); done < <(awk -F'\t' 'NR > 1 && $1 != "base" { print $1 }' "$CACHE/tiles.tsv")
fi

TODO=()
BYTES=0
for id in base "${TILES[@]}"; do
  row="$(lookup "$id")"
  [ -n "$row" ] || { echo "不明なタイル / unknown tile: $id" >&2; exit 2; }
  sha="$(cut -f3 <<<"$row")"
  if [ -f "$MARKS/$id" ] && [ "$(cat "$MARKS/$id")" = "$sha" ]; then continue; fi
  TODO+=("$id")
  BYTES=$((BYTES + $(cut -f2 <<<"$row")))
done
echo "${#TILES[@]} tiles selected, ${#TODO[@]} to download, $((BYTES / 1000000000)) GB"
if [ "$YES" = 0 ] && [ "$BYTES" -gt 20000000000 ]; then
  read -r -p "Continue? [y/N] " ans; [[ "$ans" =~ ^[yY] ]] || exit 1
fi

check() { [ -f "$1" ] && [ "$($SHA "$1" | cut -d' ' -f1)" = "$2" ]; }
n=0
for id in ${TODO[@]+"${TODO[@]}"}; do
  n=$((n + 1))
  row="$(lookup "$id")"
  file="$(cut -f1 <<<"$row")"; size="$(cut -f2 <<<"$row")"; sha="$(cut -f3 <<<"$row")"
  zip="$CACHE/$file"
  echo "[$n/${#TODO[@]}] $file ($((size / 1000000)) MB)"
  if ! check "$zip" "$sha"; then
    curl -fL --retry 5 --retry-delay 3 -C - -o "$zip" "$BASE_URL/$file" || { rm -f "$zip"; curl -fL --retry 5 -o "$zip" "$BASE_URL/$file"; }
    check "$zip" "$sha" || { rm -f "$zip"; echo "checksum mismatch: $file (deleted, run again)" >&2; exit 1; }
  fi
  if [ "$id" = base ]; then
    # never overwrite an existing level.dat (it holds your player and settings)
    tmp="$CACHE/_base"; rm -rf "$tmp"; mkdir -p "$tmp"
    unzip -q -o "$zip" -d "$tmp"
    [ -f "$WORLD_DIR/level.dat" ] && rm -f "$tmp/level.dat"
    # the spawn area regions: only where nothing is there yet
    if [ -d "$tmp/region" ]; then
      mkdir -p "$WORLD_DIR/region"
      for r in "$tmp"/region/*; do [ -e "$WORLD_DIR/region/${r##*/}" ] || cp "$r" "$WORLD_DIR/region/"; done
      rm -rf "$tmp/region"
    fi
    cp -R "$tmp"/. "$WORLD_DIR"/
    rm -rf "$tmp"
  else
    unzip -q -o "$zip" -d "$WORLD_DIR"
  fi
  echo "$sha" > "$MARKS/$id"
  [ "$KEEP" = 1 ] || rm -f "$zip"
done
echo
echo "完了 / Done: $WORLD_DIR"
echo "Minecraft Java Edition 1.21.11 → シングルプレイ / Singleplayer → 「日本 1m DEM」"
echo "ゲーム内メニュー / in-game menu: /function jp:menu"
