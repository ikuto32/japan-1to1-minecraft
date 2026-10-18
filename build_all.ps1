# 国土地理院 DEM ＋ 人工物・植生データ → 高さマップ・特徴量タイル → Minecraft ワールド を一括実行する。
# 使い方:  pwsh -File build_all.ps1 [-Meshes "5137,5138,..."] [-SkipWorld] [-SkipTerrain]
param(
    [string]$Data = 'E:\data',                        # DEM1A / DEM5A / 基本情報 / OSM / PLATEAU などを置く場所
    [string]$Work = 'E:\data\mc1m_work',              # キャッシュ・中間ファイル
    [string]$Heightmap = 'E:\data\heightmap',         # 高さマップ GeoTIFF の出力先
    [string]$Features = 'E:\data\features',           # 特徴量（土地被覆・建物・道路・鉄道・樹木）タイルの出力先
    [string]$World = 'C:\mc1m_world\japan_dem_1m',    # ワールドの出力先
    [string]$Meshes = '',                             # 第1次地域区画（省略時は前回の指定 / 既定の9区画）
    [switch]$SkipTerrain,                             # 地形（scan/lakes/heightmap）を作り直さない
    [switch]$SkipWorld
)
$ErrorActionPreference = 'Stop'
$exe = Join-Path $PSScriptRoot 'target\release\jpdem.exe'
if (-not (Test-Path $exe)) {
    Push-Location $PSScriptRoot
    cargo build --release
    Pop-Location
}
$area = @()
if ($Meshes) { $area = @('--meshes', $Meshes) }

function Step($name, [scriptblock]$body) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    Write-Host "=== $name" -ForegroundColor Cyan
    & $body
    if ($LASTEXITCODE -ne 0) { throw "$name failed (exit $LASTEXITCODE)" }
    Write-Host ("=== {0}: {1:hh\:mm\:ss}" -f $name, $sw.Elapsed) -ForegroundColor Cyan
}

if (-not $SkipTerrain) {
    # 1. zip を索引化し、版を選び（JGD2024 優先・新しい日付）、1m/5m/10m キャッシュと水域ポリゴンを作る
    Step 'scan' { & $exe scan --data $Data --work $Work }
    # 2. 5m 格子で海・湖沼・河川を判定し、水面標高と欠測補間を全域で求める
    Step 'lakes' { & $exe lakes --work $Work @area }
    # 3. 10240×10240 の高さマップタイル（中身が変わったタイルだけ書き換える）
    Step 'heightmap' { & $exe heightmap --work $Work --out $Heightmap --force @area }
}
# 4. ベクタ入力の取り込み（いずれも作業フォルダの vec_*.bin に、タイル別に格納）
Step 'kihon' { & $exe kihon --data $Data --work $Work }                 # 基盤地図情報 基本項目（建物・道路縁・道路構成線・水部構造物）
Step 'vegmap' { & $exe vegmap --data $Data --work $Work @area }         # 1/2.5万 現存植生図
Step 'plateau' { & $exe plateau --plateau (Join-Path $Data 'PLATEAU') --work $Work }   # PLATEAU 建物（LOD1/LOD2）
Step 'osm' { & $exe osm --data $Data --work $Work @area }               # OSM 道路・鉄道・ホーム・土地利用
Step 'network' { & $exe network --work $Work --heightmap $Heightmap }   # 橋・トンネルの縦断形
Step 'trees' { & $exe trees --data $Data --work $Work }                 # 静岡県 森林資源解析の単木・点群の樹冠高
if (Test-Path (Join-Path $Data 'CHMv2\chm')) {                         # Meta/WRI CHMv2 樹冠高（静岡県外の森林以外の樹木）
    Step 'chm2' { & $exe chm2 --data $Data --work $Work --heightmap $Heightmap }
}
Step 'lcmap' { & $exe lcmap --work $Work }                              # 植生・土地利用 → ブロックの対応表（無ければ作成、既存の調整は保持）
# 5. 特徴量タイル（中身が変わったタイルだけ書き換える）
Step 'features' { & $exe features --work $Work --heightmap $Heightmap --out $Features --data $Data }
# 6. ワールド（高さマップ・特徴量・対応表・橋梁データより古い region だけ作り直す）
if (-not $SkipWorld) {
    Step 'world' { & $exe world --heightmap $Heightmap --features $Features --work $Work --world $World }
}
