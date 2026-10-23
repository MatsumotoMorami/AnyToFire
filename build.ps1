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
    $dlltool = $env:LLVM_DLLTOOL
    if (-not $dlltool) { $dlltool = (Get-Command llvm-dlltool.exe -ErrorAction SilentlyContinue).Source }
    if (-not (Test-Path -LiteralPath $linker) -or -not (Test-Path -LiteralPath $dlltool)) {
        throw 'Need a MinGW gcc+dlltool installation or rust-lld+llvm-dlltool'
    }
    $env:RUSTFLAGS = "-C dlltool=$dlltool -C linker=$linker"
}
$zigCc = Join-Path $PSScriptRoot 'tools\zig-cc.cmd'
if (-not (Get-Command gcc.exe -ErrorAction SilentlyContinue)) {
    $zig = $env:ZIG_EXE
    if (-not $zig) { $zig = (Get-Command zig.exe -ErrorAction SilentlyContinue).Source }
    if (-not (Test-Path -LiteralPath $zig)) { throw 'SQLite bundled build requires Zig or MinGW gcc' }
    $env:ZIG_EXE = $zig
    $env:CC = $zigCc
    $env:AR = Join-Path $PSScriptRoot 'tools\zig-ar.cmd'
}

& $cargo test --lib
if ($LASTEXITCODE) { throw 'Tests failed' }
& $cargo test --bin AnyToFire
if ($LASTEXITCODE) { throw 'EXE tests failed' }
& $cargo build --release
if ($LASTEXITCODE) { throw 'Build failed' }

$dist = Join-Path $PSScriptRoot 'dist\AnyToFire'
New-Item -ItemType Directory -Force -Path $dist | Out-Null
Copy-Item -LiteralPath 'target\release\anytofire.dll' -Destination (Join-Path $dist 'AnyToFire.dll') -Force
Copy-Item -LiteralPath 'target\release\AnyToFire.exe' -Destination (Join-Path $dist 'AnyToFire.exe') -Force
$loader = Get-ChildItem -Path 'target\release\build' -Filter 'WebView2Loader.dll' -Recurse -File |
    Where-Object { $_.DirectoryName -match '[\\/]out[\\/]x64$' } |
    Select-Object -First 1
if (-not $loader) { throw 'WebView2Loader.dll not found in release build' }
Copy-Item -LiteralPath $loader.FullName -Destination (Join-Path $dist 'WebView2Loader.dll') -Force
$version = [regex]::Match((Get-Content -LiteralPath 'Cargo.toml' -Raw), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
if (-not $version) { throw 'Could not determine package version' }
$archive = Join-Path $PSScriptRoot "dist\AnyToFire-v$version.zip"
Compress-Archive -Path $dist -DestinationPath $archive -Force
Write-Host "Built $dist\AnyToFire.dll, $dist\AnyToFire.exe and $dist\WebView2Loader.dll"
Write-Host "Packaged $archive"

$hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText($archive + '.sha256', $hash + '  ' + [IO.Path]::GetFileName($archive) + "`n", [Text.UTF8Encoding]::new($false))
