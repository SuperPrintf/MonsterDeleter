param([switch]$Release)

$ErrorActionPreference = 'Stop'
$bridgeProject = Join-Path $PSScriptRoot 'tools\bcu-bridge\bcu-bridge.csproj'
$bridgeOutput = Join-Path $PSScriptRoot 'assets\tools\bcu-bridge'

# The installer ships the bridge as a self-contained Windows executable. Build
# it first so source-level fixes cannot be accidentally omitted from a package.
dotnet publish $bridgeProject --configuration Release --runtime win-x64 --self-contained true --output $bridgeOutput
cargo build --release
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
