$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('atf-updater-test-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $fixture | Out-Null
$scriptSource = Get-Content -LiteralPath (Join-Path $PSScriptRoot '../src/update.ps1') -Raw
foreach ($mode in @('success','invalid-entry','rollback')) {
    $root = Join-Path $fixture $mode
    $stage = Join-Path $root 'stage'
    $target = Join-Path $root 'AnyToFire'
    New-Item -ItemType Directory -Path $stage,$target | Out-Null
    foreach ($name in @('AnyToFire.exe','AnyToFire.dll','WebView2Loader.dll')) { [IO.File]::WriteAllText((Join-Path $target $name),'MZ-old') }
    [IO.File]::WriteAllText((Join-Path $root 'AnyToFire_sync.json'),'{"accessCode":"fixture-only","autoUpdate":true}')
    [IO.File]::WriteAllText((Join-Path $root 'scores.sqlite3'),'scores-sentinel')
    $zip = [IO.Compression.ZipFile]::Open((Join-Path $stage 'package.zip'),[IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($name in @('AnyToFire.exe','AnyToFire.dll','WebView2Loader.dll')) {
            $entry=$zip.CreateEntry('AnyToFire/'+$name)
            $writer=New-Object IO.StreamWriter($entry.Open())
            try { $writer.Write('MZ-new') } finally { $writer.Dispose() }
        }
        if ($mode -eq 'invalid-entry') { $zip.CreateEntry('../escape.dll') | Out-Null }
    } finally { $zip.Dispose() }
    @{target=$target;pid=2147483000} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $stage 'config.json')
    # Isolate the install logic from actual running user applications.
    $script=$scriptSource.Replace('while ((Get-Process -Id $config.pid -ErrorAction SilentlyContinue) -or (Get-Process spice64,spice -ErrorAction SilentlyContinue))','while ($false)')
    if ($mode -eq 'rollback') { $script=$script.Replace('$changed += $name', '$changed += $name; throw "fixture: injected install failure"') }
    $path=Join-Path $stage 'install.ps1';[IO.File]::WriteAllText($path,$script)
    & $path
    $expected=if ($mode -eq 'success') {'MZ-new'} else {'MZ-old'}
    foreach ($name in @('AnyToFire.exe','AnyToFire.dll','WebView2Loader.dll')) {
        if ([IO.File]::ReadAllText((Join-Path $target $name)) -ne $expected) {throw "$mode binary mismatch: $name"}
    }
    if ([IO.File]::ReadAllText((Join-Path $root 'scores.sqlite3')) -ne 'scores-sentinel') {throw 'Scores changed'}
    if ((Get-Content -LiteralPath (Join-Path $root 'AnyToFire_sync.json') -Raw | ConvertFrom-Json).accessCode -ne 'fixture-only') {throw 'Settings changed'}
    if (Test-Path -LiteralPath (Join-Path $root 'escape.dll')) {throw 'ZIP escaped destination'}
    Write-Host "PASS updater $mode, settings and scores retained"
}
