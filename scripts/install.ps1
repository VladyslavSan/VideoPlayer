# Builds a release and installs it for the current user:
#   %LOCALAPPDATA%\Programs\VideoPlayer\video-player.exe (+ libmpv-2.dll)
#   Start Menu shortcut "Video Player"
# Run it again to update. It works while the app is running: Windows can't
# overwrite a running exe or a loaded DLL, but it can rename them, so the old
# files are moved aside and the new version starts next time.
# Remove with: scripts\install.ps1 -Uninstall
param([switch]$Uninstall)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$target = Join-Path $env:LOCALAPPDATA 'Programs\VideoPlayer'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Video Player.lnk'

if ($Uninstall) {
    if (Get-Process video-player -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$target\*" }) {
        throw 'Video Player is running; close it first.'
    }
    Remove-Item $shortcut -ErrorAction SilentlyContinue
    Remove-Item $target -Recurse -ErrorAction SilentlyContinue
    Write-Output "Removed $target and the Start Menu shortcut."
    Write-Output "Your library and settings in $env:LOCALAPPDATA\VideoPlayer were kept."
    return
}

cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

New-Item -ItemType Directory -Force $target | Out-Null
# Leftovers from earlier updates; ones still in use stay locked and are kept.
Get-ChildItem $target -Filter '*.old-*' | ForEach-Object {
    try { Remove-Item $_.FullName -ErrorAction Stop } catch {}
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
foreach ($name in 'video-player.exe', 'libmpv-2.dll') {
    $source = Join-Path $root "target\release\$name"
    $dest = Join-Path $target $name
    if (Test-Path $dest) {
        if ((Get-FileHash $source).Hash -eq (Get-FileHash $dest).Hash) { continue }
        Rename-Item $dest "$name.old-$stamp"
    }
    Copy-Item $source $dest
}

$exe = Join-Path $target 'video-player.exe'
$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut($shortcut)
$link.TargetPath = $exe
$link.WorkingDirectory = $target
$link.IconLocation = "$exe,0"
$link.Description = 'Video Player'
$link.Save()

Write-Output "Installed to $exe"
Write-Output "Start Menu: Video Player"
$running = Get-Process video-player -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$target\*" }
if ($running) { Write-Output 'The app is running; restart it to use the new build.' }
