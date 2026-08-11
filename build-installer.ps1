param(
    [switch]$Release,
    [switch]$SkipBridgeBuild
)

$ErrorActionPreference = 'Stop'
$bridgeProject = Join-Path $PSScriptRoot 'tools\bcu-bridge\bcu-bridge.csproj'
$bridgeOutput = Join-Path $PSScriptRoot 'assets\tools\bcu-bridge'

# The installer ships the bridge as a self-contained Windows executable. Build
# it first so source-level fixes cannot be accidentally omitted from a package.
if ($SkipBridgeBuild) {
    if (-not (Test-Path (Join-Path $bridgeOutput 'bcu-bridge.exe'))) {
        throw 'The existing uninstall bridge binary is missing; remove -SkipBridgeBuild and build it first.'
    }
} else {
    dotnet publish $bridgeProject --configuration Release --runtime win-x64 --self-contained true --output $bridgeOutput
    if ($LASTEXITCODE -ne 0) { throw 'The uninstall bridge publish step failed.' }
}
cargo build --release --lib --bins
if ($LASTEXITCODE -ne 0) { throw 'The Rust release build failed.' }
$iscc = Get-Command iscc -ErrorAction SilentlyContinue
if (-not $iscc -and (Test-Path (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'))) {
    $isccPath = Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'
} elseif (-not $iscc -and (Test-Path (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'))) {
    $isccPath = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'
} elseif ($iscc) {
    $isccPath = $iscc.Source
} else {
    throw 'Inno Setup (iscc) was not found. Install Inno Setup 6 and run this script again.'
}
& $isccPath (Join-Path $PSScriptRoot 'installer\MonsterDeleter.iss')
if ($LASTEXITCODE -ne 0) { throw 'The Inno Setup build failed.' }
