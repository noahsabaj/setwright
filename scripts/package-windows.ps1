param([switch]$DebugBuild)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
Push-Location $repoRoot
$previousSigningKey = $env:TAURI_SIGNING_PRIVATE_KEY
try {
    if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
        $localSigningKey = Join-Path $env:LOCALAPPDATA 'SetwrightReleaseKeys\updater.key'
        if (-not (Test-Path -LiteralPath $localSigningKey)) { throw 'Set TAURI_SIGNING_PRIVATE_KEY to your existing updater signing key. Do not generate a new identity for each release.' }
        $env:TAURI_SIGNING_PRIVATE_KEY = $localSigningKey
    }
    $buildArguments = @('node_modules/@tauri-apps/cli/tauri.js', 'build', '--ci', '--bundles', 'nsis')
    if ($DebugBuild) { $buildArguments += '--debug' }
    & node @buildArguments
    if ($LASTEXITCODE -ne 0) { throw 'Signed installer build failed.' }
    $configuration = if ($DebugBuild) { 'debug' } else { 'release' }
    $packageVersion = (Get-Content package.json -Raw | ConvertFrom-Json).version
    $installer = Join-Path $repoRoot "src-tauri\target\$configuration\bundle\nsis\Setwright_${packageVersion}_x64-setup.exe"
    if (-not (Test-Path -LiteralPath $installer)) { throw "Expected x64 installer missing: $installer" }
    & node scripts/release.mjs manifest $installer (Join-Path (Split-Path $installer -Parent) 'latest.json')
    if ($LASTEXITCODE -ne 0) { throw 'Update manifest generation failed.' }
    Write-Output "Signed updater package prepared: $installer"
} finally {
    $env:TAURI_SIGNING_PRIVATE_KEY = $previousSigningKey
    Pop-Location
}
