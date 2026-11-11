param([int]$Port = 4242, [string]$Binary = '', [int]$DrainTimeoutSeconds = 60, [switch]$Restart)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path $PSScriptRoot -Parent
if (-not $Binary) { $Binary = Join-Path $taskRoot 'target\release\relay-coordinator.exe' }
$Binary = (Resolve-Path -LiteralPath $Binary).Path
$taskExpected = (& $Binary --version).Trim().Split(' ')[-1]
if ($taskExpected -notmatch '^[a-f0-9]{16}$') { throw 'Binary does not expose a build ID' }
$taskBase = "http://127.0.0.1:$Port"
$taskBootstrap = Invoke-RestMethod "$taskBase/api/bootstrap"
if ($taskBootstrap.build_id -eq $taskExpected -and -not $Restart) { Write-Output "Port $Port already runs build $taskExpected"; return }
$taskHeaders = @{'x-relay-token'=$taskBootstrap.token}
$taskData = $taskBootstrap.data_dir
if ($taskData.StartsWith('\\?\')) { $taskData = $taskData.Substring(4) }
$taskData = (Resolve-Path -LiteralPath $taskData).ProviderPath
$taskTargets = @([PSCustomObject]@{id='default';api_base='/api'})
if ($taskBootstrap.capabilities -contains 'multiple_projects') { $taskTargets = Invoke-RestMethod "$taskBase/api/projects" }
$taskBefore = @{}
$taskBackups = @{}
foreach ($taskProject in $taskTargets) {
    $taskBefore[$taskProject.id] = Invoke-RestMethod ($taskBase+$taskProject.api_base+'/state')
    $taskBackups[$taskProject.id] = Invoke-RestMethod ($taskBase+$taskProject.api_base+'/backup') -Method Post -Headers $taskHeaders
}
$taskProcessId = (Get-NetTCPConnection -LocalPort $Port -State Listen | Select-Object -First 1).OwningProcess
$taskProcess = Get-CimInstance Win32_Process -Filter "ProcessId=$taskProcessId"
if ($taskProcess.Name -ne 'relay-coordinator.exe') { throw 'Port is not owned by Relay' }
$taskDrained = $false
try {
    if ($taskBootstrap.capabilities -contains 'drain') {
        $null = Invoke-RestMethod "$taskBase/api/admin/drain" -Method Post -Headers $taskHeaders -ContentType 'application/json' -Body '{"draining":true}'
        $taskDrained = $true
    } elseif (-not $taskBefore['default'].settings.paused) { throw 'Legacy service must be paused before upgrading' }
    $taskDeadline = (Get-Date).AddSeconds($DrainTimeoutSeconds)
    do {
        $taskActive = @()
        foreach ($taskProject in $taskTargets) {
            $taskState = Invoke-RestMethod ($taskBase+$taskProject.api_base+'/state')
            $taskActive += @($taskState.attempts | Where-Object { -not $_.external -and $_.status -in @('running','cancelling') })
        }
        if (-not $taskActive.Count) { break }
        if ((Get-Date) -ge $taskDeadline) { throw 'Local runs are still active; upgrade deferred' }
        Start-Sleep -Milliseconds 500
    } while ($true)
    $taskRelease = Join-Path $taskRoot ".releases\$taskExpected"
    $null = New-Item -ItemType Directory -Path $taskRelease -Force
    $taskExecutable = Join-Path $taskRelease 'relay-coordinator.exe'
    if (Test-Path -LiteralPath $taskExecutable) {
        if ((Get-FileHash -LiteralPath $Binary).Hash -ne (Get-FileHash -LiteralPath $taskExecutable).Hash) { throw 'Versioned release exists with different bytes' }
    } else { Copy-Item -LiteralPath $Binary -Destination $taskExecutable }
    foreach ($taskProject in $taskTargets) { $taskBefore[$taskProject.id] = Invoke-RestMethod ($taskBase+$taskProject.api_base+'/state') }
    Stop-Process -Id $taskProcessId
    Wait-Process -Id $taskProcessId -ErrorAction SilentlyContinue
    $taskOut = Join-Path $taskData "service-$taskExpected.out.log"
    $taskErr = Join-Path $taskData "service-$taskExpected.err.log"
    $taskArguments = @('--port', "$Port", '--data-dir', ('"'+$taskData+'"'))
    $taskStarted = Start-Process -FilePath $taskExecutable -ArgumentList $taskArguments -WorkingDirectory $taskRoot -WindowStyle Hidden -RedirectStandardOutput $taskOut -RedirectStandardError $taskErr -PassThru
    $taskDeadline = (Get-Date).AddSeconds(20)
    do {
        try { $taskAfterBoot = Invoke-RestMethod "$taskBase/api/bootstrap" -TimeoutSec 2; break } catch {
            if ($taskStarted.HasExited -or (Get-Date) -ge $taskDeadline) { throw 'Updated Relay failed to start; backup and previous executable are retained' }
            Start-Sleep -Milliseconds 200
        }
    } while ($true)
    if ($taskAfterBoot.build_id -ne $taskExpected) { throw 'Running build does not match tested binary' }
    foreach ($taskProject in $taskTargets) {
        $taskAfter = Invoke-RestMethod ($taskBase+$taskProject.api_base+'/state')
        $taskPrior = $taskBefore[$taskProject.id]
        foreach ($taskCollection in @('tasks','agents','attempts','events','artifacts')) {
            if (@($taskAfter.$taskCollection).Count -lt @($taskPrior.$taskCollection).Count) { throw "Upgrade lost $taskCollection records in $($taskProject.id)" }
            $taskAfterIds = @($taskAfter.$taskCollection | ForEach-Object { $_.id })
            foreach ($taskRecord in $taskPrior.$taskCollection) {
                if ($taskRecord.id -notin $taskAfterIds) { throw "Upgrade lost a $taskCollection identity in $($taskProject.id)" }
            }
        }
    }
    [PSCustomObject]@{Port=$Port;Build=$taskAfterBoot.build_id;ProcessId=$taskStarted.Id;Projects=$taskTargets.Count;Backups=$taskBackups} | ConvertTo-Json -Depth 5
} catch {
    if ($taskDrained) {
        try { $null=Invoke-RestMethod "$taskBase/api/admin/drain" -Method Post -Headers $taskHeaders -ContentType 'application/json' -Body '{"draining":false}' } catch {}
    }
    throw
}
