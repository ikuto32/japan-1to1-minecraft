<#
  日本 1:1 Minecraft ワールド — 選んだタイル（10,240 ブロック四方）だけをダウンロードして展開します。
  Japan 1:1 Minecraft world — downloads and unpacks only the tiles (10,240 blocks square) you choose.

  例 / Examples
    powershell -ExecutionPolicy Bypass -File download.ps1 -Tiles x-20480_z112640,x-10240_z112640
    powershell -ExecutionPolicy Bypass -File download.ps1 -TileFile tiles.txt -Dest D:\minecraft\saves
    powershell -ExecutionPolicy Bypass -File download.ps1 -Tiles all

  タイルの ID はタイル選択ページで選んでコピーできます。何度でも実行でき、追加したタイルだけを取得します。
  Pick tiles and copy their IDs on the tile selection page. Safe to re-run: only missing tiles are fetched.
#>
[CmdletBinding()]
param(
    [string[]]$Tiles = @(),
    [string]$TileFile = '',
    [string]$Dest = '',          # 既定 / default: %APPDATA%\.minecraft\saves
    [string]$Repo = '__REPO__',
    [string]$Tag = '__TAG__',
    [switch]$KeepZips,           # ダウンロードした zip を残す / keep the downloaded zips
    [switch]$Yes,                # 確認しない / don't ask for confirmation
    [string]$BaseUrl = ''        # 取得元を変える（ミラー・テスト用）/ override the download location
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12 } catch {}

$World = '__WORLD__'
if (-not $BaseUrl) { $BaseUrl = "https://github.com/$Repo/releases/download/$Tag" }
$BaseUrl = $BaseUrl.TrimEnd('/')
if (-not $Dest) { $Dest = Join-Path $env:APPDATA '.minecraft\saves' }
$WorldDir = Join-Path $Dest $World
$Cache = Join-Path $Dest "_$($World)_downloads"
$Marks = Join-Path $WorldDir '.downloaded'
New-Item -ItemType Directory -Force -Path $WorldDir, $Cache, $Marks | Out-Null
$haveCurl = [bool](Get-Command curl.exe -ErrorAction SilentlyContinue)
$haveTar = [bool](Get-Command tar.exe -ErrorAction SilentlyContinue)

function Get-Url([string]$url, [string]$out, [switch]$Resume) {
    if ($haveCurl) {
        $a = @('-fL', '--retry', '5', '--retry-delay', '3', '-o', $out)
        if ($Resume) { $a += @('-C', '-') }
        & curl.exe @a $url
        if ($LASTEXITCODE -ne 0) { throw "download failed ($LASTEXITCODE): $url" }
    } else {
        Invoke-WebRequest -Uri $url -OutFile $out -UseBasicParsing
    }
}

function Test-Hash([string]$path, [string]$sha) {
    if (-not (Test-Path $path)) { return $false }
    if (-not $sha) { return $true }
    return ((Get-FileHash -Algorithm SHA256 -Path $path).Hash.ToLower() -eq $sha.ToLower())
}

function Get-Asset($row) {
    $zip = Join-Path $Cache $row.file
    if (Test-Hash $zip $row.sha256) { return $zip }
    $url = "$BaseUrl/$($row.file)"
    try {
        Get-Url $url $zip -Resume:(Test-Path $zip)
    } catch {
        Remove-Item -Force -ErrorAction SilentlyContinue $zip
        Get-Url $url $zip
    }
    if (-not (Test-Hash $zip $row.sha256)) {
        Remove-Item -Force -ErrorAction SilentlyContinue $zip
        throw "checksum mismatch: $($row.file) (deleted, please run again)"
    }
    return $zip
}

function Expand-Zip([string]$zip, [string]$dir) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    if ($haveTar) {
        & tar.exe -xf $zip -C $dir
        if ($LASTEXITCODE -ne 0) { throw "unpack failed: $zip" }
    } else {
        Expand-Archive -Path $zip -DestinationPath $dir -Force
    }
}

# ---- manifest
$tsv = Join-Path $Cache 'tiles.tsv'
Write-Host "manifest: $BaseUrl/tiles.tsv"
Get-Url "$BaseUrl/tiles.tsv" $tsv
$rows = @(Import-Csv -Path $tsv -Delimiter "`t")
$byId = @{}
foreach ($r in $rows) { $byId[$r.id] = $r }

# ---- selection
$want = @()
foreach ($t in $Tiles) { $want += ($t -split '[,;\s]+') }
if ($TileFile) { $want += (Get-Content -Path $TileFile | ForEach-Object { $_ -split '[,;\s]+' }) }
$want = @($want | Where-Object { $_ } | ForEach-Object { $_.Trim() } | Select-Object -Unique)
if ($want -contains 'all') { $want = @($rows | Where-Object { $_.id -ne 'base' } | ForEach-Object { $_.id }) }
if ($want.Count -eq 0) { throw 'タイルを指定してください（-Tiles または -TileFile）/ specify tiles with -Tiles or -TileFile' }
$unknown = @($want | Where-Object { -not $byId.ContainsKey($_) })
if ($unknown.Count) { throw ("不明なタイル / unknown tiles: " + ($unknown -join ', ')) }

$todo = @()
foreach ($id in @('base') + $want) {
    $r = $byId[$id]
    $mark = Join-Path $Marks $id
    if ((Test-Path $mark) -and ((Get-Content -Raw $mark).Trim() -eq $r.sha256)) { continue }
    $todo += $r
}
$bytes = ($todo | Measure-Object -Property size -Sum).Sum
if (-not $bytes) { $bytes = 0 }
Write-Host ("{0} tiles selected, {1} to download, {2:N1} GB" -f $want.Count, $todo.Count, ($bytes / 1e9))
$drive = (Get-Item $Dest).PSDrive
if ($drive -and $drive.Free -and $drive.Free -lt $bytes * 2.4) {
    Write-Warning ("空き容量が足りない可能性があります / low disk space: {0:N1} GB free" -f ($drive.Free / 1e9))
}
if (-not $Yes -and $bytes -gt 20e9) {
    $ans = Read-Host 'Continue? [y/N]'
    if ($ans -notmatch '^[yY]') { exit 1 }
}

# ---- download and unpack
$i = 0
foreach ($r in $todo) {
    $i++
    Write-Host ("[{0}/{1}] {2} ({3:N2} GB)" -f $i, $todo.Count, $r.file, ($r.size / 1e9))
    $zip = Get-Asset $r
    if ($r.id -eq 'base') {
        # never overwrite an existing level.dat (it holds your player and settings)
        $tmp = Join-Path $Cache '_base'
        if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
        Expand-Zip $zip $tmp
        Get-ChildItem -Force $tmp | ForEach-Object {
            $target = Join-Path $WorldDir $_.Name
            if ($_.Name -eq 'level.dat' -and (Test-Path $target)) { return }
            if ($_.Name -eq 'region') {
                # the spawn area regions: only where nothing is there yet
                New-Item -ItemType Directory -Force -Path $target | Out-Null
                Get-ChildItem $_.FullName | ForEach-Object {
                    $d = Join-Path $target $_.Name
                    if (-not (Test-Path $d)) { Copy-Item $_.FullName $d }
                }
                return
            }
            if ($_.PSIsContainer -and (Test-Path $target)) { Copy-Item -Recurse -Force (Join-Path $_.FullName '*') $target }
            else { Copy-Item -Recurse -Force $_.FullName $WorldDir }
        }
        Remove-Item -Recurse -Force $tmp
    } else {
        Expand-Zip $zip $WorldDir
    }
    Set-Content -Path (Join-Path $Marks $r.id) -Value $r.sha256
    if (-not $KeepZips) { Remove-Item -Force $zip }
}
Write-Host ''
Write-Host "完了 / Done: $WorldDir"
Write-Host 'Minecraft Java Edition 1.21.11 → シングルプレイ / Singleplayer → 「日本 1m DEM」'
Write-Host 'ゲーム内メニュー / in-game menu: /function jp:menu'
