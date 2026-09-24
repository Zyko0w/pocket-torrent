# Construit une version signée de Pocket Torrent et prépare les fichiers de la release GitHub.
#
# Usage (dans une fenêtre PowerShell, pour pouvoir taper le mot de passe) :
#   powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1 -Notes "Ce qui change"
#
# Le mot de passe de la clé est demandé sans écho, gardé seulement le temps du build, jamais écrit.
# Résultat dans src-tauri\target\release\publication\v<version>\ : installateur (sans espace dans
# le nom, GitHub les remplace), sa signature et latest.json, prêts pour `gh release create`.

param(
    [Parameter(Mandatory = $true)][string]$Notes,
    [string]$KeyPath = "$env:USERPROFILE\.tauri\torrent-cloud.key",
    [string]$Repo = "Zyko0w/pocket-torrent-releases"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Get-Content "src-tauri\tauri.conf.json" -Raw | ConvertFrom-Json).version
if (-not (Test-Path $KeyPath)) { throw "Clé de signature introuvable : $KeyPath" }
Write-Host "Pocket Torrent $version" -ForegroundColor Cyan

$secure = Read-Host "Mot de passe de la clé de signature" -AsSecureString
$bstr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
try {
    $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $KeyPath -Raw
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($bstr)
    npx tauri build
    if ($LASTEXITCODE -ne 0) { throw "le build a échoué (mot de passe incorrect ?)" }
}
finally {
    # rien ne reste dans l'environnement ni en mémoire non gérée
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr)
    Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY, Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
}

$bundle = "src-tauri\target\release\bundle\nsis"
$installer = "PocketTorrent_${version}_x64-setup.exe"
$out = "src-tauri\target\release\publication\v$version"
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item "$bundle\Pocket Torrent_${version}_x64-setup.exe" "$out\$installer"
Copy-Item "$bundle\Pocket Torrent_${version}_x64-setup.exe.sig" "$out\$installer.sig"

$latest = [ordered]@{
    version   = $version
    notes     = $Notes
    pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = [ordered]@{
        "windows-x86_64" = [ordered]@{
            signature = (Get-Content "$out\$installer.sig" -Raw).Trim()
            url       = "https://github.com/$Repo/releases/download/v$version/$installer"
        }
    }
}
# UTF-8 sans BOM : l'updater lit du JSON strict
[IO.File]::WriteAllText("$root\$out\latest.json", ($latest | ConvertTo-Json -Depth 5), (New-Object Text.UTF8Encoding $false))

# calcul direct (.NET) : Get-FileHash manque quand Windows PowerShell est lancé depuis PowerShell 7
$sha = [Security.Cryptography.SHA256]::Create()
$stream = [IO.File]::OpenRead("$root\$out\$installer")
try { $hash = -join ($sha.ComputeHash($stream) | ForEach-Object { $_.ToString("x2") }) }
finally { $stream.Dispose(); $sha.Dispose() }
Write-Host "`nPrêt dans $out" -ForegroundColor Green
Write-Host "SHA-256 : $hash"
