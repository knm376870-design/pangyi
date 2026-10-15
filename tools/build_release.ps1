[CmdletBinding()]
param(
    [string]$LlvmMingwBin,
    [string]$BuildDir,
    [string]$OutputDir,
    [switch]$Offline
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
if (-not $LlvmMingwBin) { $LlvmMingwBin = Join-Path $repoRoot 'tools\llvm-mingw\bin' }
if (-not $BuildDir) { $BuildDir = Join-Path $repoRoot 'ime\target' }
if (-not $OutputDir) { $OutputDir = Join-Path $repoRoot 'dist' }
$BuildDir = [IO.Path]::GetFullPath($BuildDir)
$OutputDir = [IO.Path]::GetFullPath($OutputDir)
$LlvmMingwBin = [IO.Path]::GetFullPath($LlvmMingwBin)
if ($BuildDir -match '[^\x00-\x7F]') {
    throw 'Windows GNU requires an ASCII-only build output path. Set -BuildDir to a writable ASCII path.'
}

$compiler32 = Join-Path $LlvmMingwBin 'i686-w64-mingw32-gcc.exe'
$windres32 = Join-Path $LlvmMingwBin 'i686-w64-mingw32-windres.exe'
$windres64 = Join-Path $LlvmMingwBin 'x86_64-w64-mingw32-windres.exe'
foreach ($requiredFile in @($compiler32, $windres32, $windres64, (Join-Path $repoRoot 'ime\build\pangyi.ico'))) {
    if (-not (Test-Path -LiteralPath $requiredFile -PathType Leaf)) { throw "Missing build input: $requiredFile" }
}
$manifest = Join-Path $repoRoot 'ime\Cargo.toml'
$versionMatch = [regex]::Match([IO.File]::ReadAllText($manifest), '(?m)^version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) { throw 'Package version was not found in Cargo.toml.' }
$version = $versionMatch.Groups[1].Value
$payloadDir = Join-Path $repoRoot 'ime\payload'
$supportDir = Join-Path $BuildDir 'support'
foreach ($dir in @($BuildDir, $OutputDir, $payloadDir, $supportDir)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
}

function Invoke-CargoBuild([string[]]$BuildArgs) {
    $cargoArgs = @('build', '--manifest-path', $manifest, '--release', '--locked') + $BuildArgs
    if ($Offline) { $cargoArgs += '--offline' }
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "Cargo build failed: $($BuildArgs -join ' ')" }
}

function Assert-PeMachine([string]$FilePath, [uint16]$Expected) {
    $reader = [IO.BinaryReader]::new([IO.File]::OpenRead($FilePath))
    try {
        if ($reader.ReadUInt16() -ne 0x5A4D) { throw "Invalid MZ header: $FilePath" }
        $reader.BaseStream.Position = 0x3C
        $peOffset = $reader.ReadInt32()
        $reader.BaseStream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne $Expected) {
            throw "Unexpected PE architecture: $FilePath"
        }
    } finally { $reader.Dispose() }
}

# Keep the host linker PATH unchanged: mixing llvm-mingw into x64 builds can break linking.
$envNames = @('CARGO_TARGET_DIR', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER', 'PANGYI_WINDRES', 'TMP', 'TEMP', 'LOCALAPPDATA')
$savedEnv = @{}
foreach ($envName in $envNames) { $savedEnv[$envName] = [Environment]::GetEnvironmentVariable($envName, 'Process') }
try {
    $env:CARGO_TARGET_DIR = $BuildDir
    # Some sandbox TEMP mappings reject llvm-rc's temporary-file rename operation.
    $toolTemp = Join-Path $BuildDir 'temp'
    New-Item -ItemType Directory -Path $toolTemp -Force | Out-Null
    $env:TMP = $toolTemp
    $env:TEMP = $toolTemp
    $env:LOCALAPPDATA = Join-Path $supportDir 'probe-profile'
    [Environment]::SetEnvironmentVariable('RUSTFLAGS', $null, 'Process')
    [Environment]::SetEnvironmentVariable('CARGO_ENCODED_RUSTFLAGS', $null, 'Process')
    Copy-Item -LiteralPath (Join-Path $repoRoot 'ime\build\pangyi.ico') -Destination (Join-Path $payloadDir 'pangyi.ico') -Force

    $env:PANGYI_WINDRES = $windres64
    Invoke-CargoBuild @('--target', 'x86_64-pc-windows-gnu', '--lib')
    $dll64 = Join-Path $BuildDir 'x86_64-pc-windows-gnu\release\pangyi_ime.dll'
    Assert-PeMachine $dll64 0x8664
    Copy-Item -LiteralPath $dll64 -Destination (Join-Path $payloadDir 'pangyi_ime.dll') -Force
    Invoke-CargoBuild @('--target', 'x86_64-pc-windows-gnu', '--bin', 'stability_probe')
    & (Join-Path $BuildDir 'x86_64-pc-windows-gnu\release\stability_probe.exe') $dll64
    if ($LASTEXITCODE -ne 0) { throw '64-bit DLL runtime probe failed.' }

    $stubSource = Join-Path $supportDir 'x86-stub.c'
    $stubObject = Join-Path $supportDir 'x86-stub.o'
    Copy-Item -LiteralPath (Join-Path $repoRoot 'ime\build\x86-stub.c') -Destination $stubSource -Force
    & $compiler32 -c $stubSource -o $stubObject
    if ($LASTEXITCODE -ne 0) { throw 'Failed to compile the x86 linker stub.' }
    $env:CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER = $compiler32
    $env:PANGYI_WINDRES = $windres32
    $env:CARGO_ENCODED_RUSTFLAGS = '-C' + [char]0x1F + 'link-arg=' + $stubObject
    Invoke-CargoBuild @('--target', 'i686-pc-windows-gnu', '--lib')
    $dll32 = Join-Path $BuildDir 'i686-pc-windows-gnu\release\pangyi_ime.dll'
    Assert-PeMachine $dll32 0x014C
    Copy-Item -LiteralPath $dll32 -Destination (Join-Path $payloadDir 'pangyi_ime-x86.dll') -Force
    Invoke-CargoBuild @('--target', 'i686-pc-windows-gnu', '--bin', 'stability_probe')
    & (Join-Path $BuildDir 'i686-pc-windows-gnu\release\stability_probe.exe') $dll32
    if ($LASTEXITCODE -ne 0) { throw '32-bit DLL runtime probe failed.' }

    [Environment]::SetEnvironmentVariable('RUSTFLAGS', $null, 'Process')
    [Environment]::SetEnvironmentVariable('CARGO_ENCODED_RUSTFLAGS', $null, 'Process')
    $env:PANGYI_WINDRES = $windres64
    Invoke-CargoBuild @('--target', 'x86_64-pc-windows-gnu', '--bin', 'setup')
    $setup = Join-Path $BuildDir 'x86_64-pc-windows-gnu\release\setup.exe'
    Assert-PeMachine $setup 0x8664
    $versionInfo = [Diagnostics.FileVersionInfo]::GetVersionInfo($setup)
    if ($versionInfo.FileVersion -ne $version -or $versionInfo.FileDescription -notmatch '旁译') {
        throw 'Installer version or Chinese Windows resources are missing. Check windres output.'
    }
    $installer = Join-Path $OutputDir "旁译-v$version.exe"
    Copy-Item -LiteralPath $setup -Destination $installer -Force
    $checksum = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$installer.sha256", "$checksum  $([IO.Path]::GetFileName($installer))`n", [Text.UTF8Encoding]::new($false))
    [pscustomobject]@{ Installer = $installer; Version = $version; Sha256 = $checksum; Dll64 = $dll64; Dll32 = $dll32 }
} finally {
    foreach ($envName in $envNames) { [Environment]::SetEnvironmentVariable($envName, $savedEnv[$envName], 'Process') }
}
