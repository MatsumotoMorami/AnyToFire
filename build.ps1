$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

$cargo = (Get-Command cargo.exe -ErrorAction SilentlyContinue).Source
if (-not $cargo) { $cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
if (-not (Test-Path -LiteralPath $cargo)) { throw 'Rust cargo.exe not found' }

# Standard MinGW installations can build without overrides. The local Rust GNU
# toolchain has neither gcc nor dlltool on PATH, so use rust-lld and llvm-dlltool.
if (-not (Get-Command gcc.exe -ErrorAction SilentlyContinue) -or -not (Get-Command dlltool.exe -ErrorAction SilentlyContinue)) {
    $rustc = Join-Path (Split-Path $cargo) 'rustc.exe'
    $sysroot = (& $rustc --print sysroot).Trim()
    $linker = Join-Path $sysroot 'lib\rustlib\x86_64-pc-windows-gnu\bin\rust-lld.exe'
    $dlltool = (Get-Command llvm-dlltool.exe -ErrorAction SilentlyContinue).Source
    if (-not $dlltool) {
        $dlltool = 'C:\Users\mbhxx\Documents\Codex\2026-10-15\qing\work\toolchains\android\android-ndk-r27d\toolchains\llvm\prebuilt\windows-x86_64\bin\llvm-dlltool.exe'
    }
    if (-not (Test-Path -LiteralPath $linker) -or -not (Test-Path -LiteralPath $dlltool)) {
        throw 'Need a MinGW gcc+dlltool installation or rust-lld+llvm-dlltool'
    }
    $env:RUSTFLAGS = "-C dlltool=$dlltool -C linker=$linker"
}

& $cargo test --lib
if ($LASTEXITCODE) { throw 'Tests failed' }
& $cargo build --release
if ($LASTEXITCODE) { throw 'Build failed' }

$dist = Join-Path $PSScriptRoot 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null
Copy-Item -LiteralPath 'target\release\anytofire.dll' -Destination (Join-Path $dist 'AnyToFire.dll') -Force
Copy-Item -LiteralPath 'target\release\AnyToFire.exe' -Destination (Join-Path $dist 'AnyToFire.exe') -Force
Write-Host "Built $dist\AnyToFire.dll and $dist\AnyToFire.exe"
