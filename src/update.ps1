$ErrorActionPreference = 'Stop'
$stage = $PSScriptRoot
$changed = @()
try {
    $config = Get-Content -LiteralPath (Join-Path $stage 'config.json') -Raw | ConvertFrom-Json
    $target = [IO.Path]::GetFullPath($config.target)
    $deadline = (Get-Date).AddHours(24)
    while ((Get-Process -Id $config.pid -ErrorAction SilentlyContinue) -or (Get-Process spice64,spice -ErrorAction SilentlyContinue)) {
        if ((Get-Date) -gt $deadline) { throw 'Update timed out while waiting for game/app exit' }
        Start-Sleep -Seconds 3
    }
    $game = if ((Split-Path $target -Leaf) -ieq 'AnyToFire') { Split-Path $target -Parent } else { $target }
    $settings = Join-Path $game 'AnyToFire_sync.json'
    if (Test-Path -LiteralPath $settings) {
        $saved = Get-Content -LiteralPath $settings -Raw | ConvertFrom-Json
        if ($saved.autoUpdate -eq $false) { return }
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead((Join-Path $stage 'package.zip'))
    $allowed = @('AnyToFire.exe','AnyToFire.dll','WebView2Loader.dll')
    $entries = @{}
    try {
        foreach ($entry in $zip.Entries) {
            if ($entry.FullName.EndsWith('/')) { continue }
            if ($entry.FullName -notmatch '^AnyToFire/(AnyToFire\.exe|AnyToFire\.dll|WebView2Loader\.dll)$') { throw 'Unexpected update archive entry' }
            if ($entries.ContainsKey($entry.Name) -or $entry.Length -gt 100MB) { throw 'Invalid update archive entry' }
            $entries[$entry.Name] = $entry
        }
        if ($entries.Count -ne 3) { throw 'Incomplete update package' }
        foreach ($name in $allowed) {
            $path = Join-Path $stage $name
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entries[$name],$path,$false)
            $stream = [IO.File]::OpenRead($path)
            try { if ($stream.ReadByte() -ne 77 -or $stream.ReadByte() -ne 90) { throw 'Invalid PE binary' } } finally { $stream.Dispose() }
        }
    } finally { $zip.Dispose() }
    $backup = Join-Path $target ('update-backup-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
    New-Item -ItemType Directory -Path $backup | Out-Null
    foreach ($name in $allowed) { if (Test-Path -LiteralPath (Join-Path $target $name)) { Copy-Item -LiteralPath (Join-Path $target $name) -Destination (Join-Path $backup $name) } }
    foreach ($name in $allowed) {
        $destination = Join-Path $target $name
        $pending = $destination + '.update'
        Copy-Item -LiteralPath (Join-Path $stage $name) -Destination $pending -Force
        Move-Item -LiteralPath $pending -Destination $destination -Force
        $changed += $name
    }
    'Update installed. Previous binaries: ' + $backup | Set-Content -LiteralPath (Join-Path $target 'update.log')
} catch {
    $failure = $_.Exception.Message
    foreach ($name in $changed) {
        try { Copy-Item -LiteralPath (Join-Path $backup $name) -Destination (Join-Path $target $name) -Force } catch {}
    }
    ('Update failed: ' + $failure) | Set-Content -LiteralPath (Join-Path $stage 'update-error.log')
    if ($target) { ('Update failed: ' + $failure) | Set-Content -LiteralPath (Join-Path $target 'update.log') }
}
