param([Parameter(Mandatory=$true)][string]$Source,[Parameter(Mandatory=$true)][string]$Cache)
$ErrorActionPreference='Stop'
$Source=(Resolve-Path -LiteralPath $Source).Path
$Cache=[IO.Path]::GetFullPath($Cache)
if($Cache.StartsWith($Source.TrimEnd('\')+'\',[StringComparison]::OrdinalIgnoreCase) -or $Cache.Equals($Source,[StringComparison]::OrdinalIgnoreCase)) { throw 'Build cache must be outside source' }
New-Item -ItemType Directory -Force $Cache | Out-Null
$guard=[IO.File]::Open((Join-Path $Cache 'build.lock'),'OpenOrCreate','ReadWrite','None')
try {
    $timer=[Diagnostics.Stopwatch]::StartNew()
    function HashFiles($root,$files) {
        $hash=[Security.Cryptography.IncrementalHash]::CreateHash([Security.Cryptography.HashAlgorithmName]::SHA256)
        foreach($file in ($files | Sort-Object FullName)) {
            $hash.AppendData([Text.Encoding]::UTF8.GetBytes([IO.Path]::GetRelativePath($root,$file.FullName)))
            $hash.AppendData([byte[]]@(0))
            $hash.AppendData([IO.File]::ReadAllBytes($file.FullName))
        }
        $result=[Convert]::ToHexString($hash.GetHashAndReset()).ToLowerInvariant()
        $hash.Dispose()
        return $result
    }
    function SyncFiles($root,$destination,$files) {
        foreach($file in $files) {
            $target=Join-Path $destination ([IO.Path]::GetRelativePath($root,$file.FullName))
            New-Item -ItemType Directory -Force ([IO.Path]::GetDirectoryName($target)) | Out-Null
            if(!(Test-Path -LiteralPath $target) -or (Get-FileHash -LiteralPath $target).Hash -ne (Get-FileHash -LiteralPath $file.FullName).Hash) { Copy-Item -LiteralPath $file.FullName -Destination $target -Force }
        }
    }
    $frontendFiles=@(Get-ChildItem -LiteralPath (Join-Path $Source 'frontend/src') -File -Recurse)+@(Get-Item (Join-Path $Source 'frontend/package.json'),(Join-Path $Source 'frontend/pnpm-lock.yaml'),(Join-Path $Source 'frontend/index.html'),(Join-Path $Source 'frontend/vite.config.js'))
    $frontendSha=HashFiles $Source $frontendFiles
    $ui=Join-Path $Cache ('frontend-'+$frontendSha)
    $snapshot=Join-Path $Cache 'source'
    $web=Join-Path $snapshot 'web'
    $rebuilt=!(Test-Path -LiteralPath (Join-Path $ui 'complete'))
    if($rebuilt) {
        $pnpm=if($env:COORDINATOR_PNPM) { $env:COORDINATOR_PNPM } else { (Get-Command pnpm -ErrorAction SilentlyContinue).Source }
        if(!$pnpm) { throw 'Frontend changed and pnpm is unavailable; add pnpm to PATH or set COORDINATOR_PNPM, or deploy a prepared candidate' }
        SyncFiles (Join-Path $Source 'frontend') $ui $frontendFiles
        & $pnpm --dir $ui install --frozen-lockfile --prefer-offline --reporter append-only 2>&1 | ForEach-Object { [Console]::Error.WriteLine($_.ToString()) }
        if($LASTEXITCODE -ne 0) { throw 'Frontend install failed' }
        & $pnpm --dir $ui exec vite build 2>&1 | ForEach-Object { [Console]::Error.WriteLine($_.ToString()) }
        if($LASTEXITCODE -ne 0) { throw 'Frontend build failed' }
        Set-Content -LiteralPath (Join-Path $ui 'complete') -Value $frontendSha
    }
    $frontendMs=$timer.ElapsedMilliseconds
    $assets=@(Get-Item (Join-Path $ui 'dist/index.html'),(Join-Path $ui 'dist/app.js'),(Join-Path $ui 'dist/style.css'))
    SyncFiles (Join-Path $ui 'dist') $web $assets
    $assetHash=[Security.Cryptography.IncrementalHash]::CreateHash([Security.Cryptography.HashAlgorithmName]::SHA256)
    foreach($name in @('index.html','app.js','style.css')) {
        $bytes=[IO.File]::ReadAllBytes((Join-Path $web $name))
        $assetHash.AppendData([BitConverter]::GetBytes([uint64]$bytes.Length))
        $assetHash.AppendData($bytes)
    }
    $frontend='sha256:'+([Convert]::ToHexString($assetHash.GetHashAndReset()).ToLowerInvariant())
    $assetHash.Dispose()
    $rustFiles=@(Get-ChildItem -LiteralPath (Join-Path $Source 'src') -File -Recurse)+@(Get-Item (Join-Path $Source 'Cargo.toml'),(Join-Path $Source 'Cargo.lock'),(Join-Path $Source 'rust-toolchain.toml'),(Join-Path $Source 'scripts/recording.rs'))
    $sourceSha=HashFiles $Source ($rustFiles+$frontendFiles)
    SyncFiles $Source $snapshot $rustFiles
    $expected=@($rustFiles | ForEach-Object { [IO.Path]::GetRelativePath($Source,$_.FullName) })
    Get-ChildItem -LiteralPath (Join-Path $snapshot 'src') -File -Recurse | Where-Object { $expected -notcontains [IO.Path]::GetRelativePath($snapshot,$_.FullName) } | Remove-Item -Force
    $env:COORDINATOR_FRONTEND_SHA=$frontendSha
    $env:COORDINATOR_SOURCE_SHA=$sourceSha
    $env:COORDINATOR_SOURCE_ROOT=$Source
    $env:CARGO_TARGET_DIR=Join-Path $Cache 'target'
    $cargo=Get-Command cargo -ErrorAction SilentlyContinue
    if($cargo) { $cargo=$cargo.Source } else { $cargo=Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
    $env:PATH=(Join-Path $env:USERPROFILE '.cargo/bin')+';'+$env:PATH
    Push-Location $snapshot
    try {
        & $cargo build --locked --release --bin agent-coordinator 2>&1 | ForEach-Object { [Console]::Error.WriteLine($_.ToString()) }
        if($LASTEXITCODE -ne 0) { throw 'Rust build failed' }
    } finally { Pop-Location }
    if((HashFiles $Source $frontendFiles) -ne $frontendSha -or (HashFiles $Source ($rustFiles+$frontendFiles)) -ne $sourceSha) { throw 'Source changed during build; retry' }
    @{mode='source';candidate=(Join-Path $env:CARGO_TARGET_DIR 'release/agent-coordinator.exe');source=$sourceSha;frontendSource=$frontendSha;frontend=$frontend;frontendRebuilt=$rebuilt;frontendMs=$frontendMs;rustMs=($timer.ElapsedMilliseconds-$frontendMs);buildMs=$timer.ElapsedMilliseconds} | ConvertTo-Json -Compress
} finally { $guard.Dispose() }


