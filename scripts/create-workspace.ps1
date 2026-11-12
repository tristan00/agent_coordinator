param(
    [Parameter(Mandatory = $true)][string[]]$Workspace,
    [string]$CoordinatorUrl = 'http://127.0.0.1:4242',
    [Nullable[int]]$Seed,
    [ValidateSet('', 'poll', 'events')][string]$Updates = ''
)

$ErrorActionPreference = 'Stop'
$baseUrl = $CoordinatorUrl.TrimEnd('/')
$project = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$random = if ($null -ne $Seed) { [Random]::new([int]$Seed) } else { [Random]::new() }
$ranges = [ordered]@{
    words = @(60, 240)
    titleWords = @(6, 20)
    criteria = @(1, 6)
    summaryWords = @(20, 80)
    findings = @(1, 6)
    findingWords = @(15, 60)
}
function Get-WorkspaceKey([string]$Path) {
    $Path.Replace('\\?\', '').TrimEnd('\', '/').ToLowerInvariant()
}

$state = Invoke-RestMethod -Uri "$baseUrl/api/state" -TimeoutSec 15
$config = $state.config
if ($null -eq $config.workspaces) { $config | Add-Member -NotePropertyName workspaces -NotePropertyValue ([pscustomobject]@{}) -Force }
$known = @{}
foreach ($property in $config.workspaces.PSObject.Properties) { $known[(Get-WorkspaceKey $property.Name)] = $true }
$paths = foreach ($item in $Workspace) {
    if ([string]::IsNullOrWhiteSpace($item)) { throw 'Workspace paths must not be empty' }
    $path = [IO.Path]::GetFullPath($item)
    if ($path.Equals($project, [StringComparison]::OrdinalIgnoreCase) -or $path.StartsWith($project + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw "Workspace data must be outside the coordinator source directory: $path"
    }
    if (Test-Path -LiteralPath $path) { throw "Workspace already exists: $path" }
    $key = Get-WorkspaceKey $path
    if ($known.ContainsKey($key)) { throw "Workspace is already configured or repeated: $path" }
    $known[$key] = $true
    $path
}
$created = foreach ($path in $paths) {
    [IO.Directory]::CreateDirectory($path) | Out-Null
    $limits = [ordered]@{}
    foreach ($entry in $ranges.GetEnumerator()) { $limits[$entry.Key] = $random.Next($entry.Value[0], $entry.Value[1] + 1) }
    $mode = if ($Updates) { $Updates } else { @('poll', 'events')[$random.Next(0, 2)] }
    $config.workspaces | Add-Member -NotePropertyName $path -NotePropertyValue ([pscustomobject]@{ limits = [pscustomobject]$limits; updates = $mode })
    [pscustomobject]@{ workspace = $path; limits = [pscustomobject]$limits; updates = $mode }
}
Invoke-RestMethod -Uri "$baseUrl/api/config" -Method Put -ContentType 'application/json' -Body ($config | ConvertTo-Json -Depth 100) -TimeoutSec 15 | Out-Null
$verified = Invoke-RestMethod -Uri "$baseUrl/api/state" -TimeoutSec 15
foreach ($item in $created) {
    $saved = @($verified.config.workspaces.PSObject.Properties | Where-Object { (Get-WorkspaceKey $_.Name) -eq (Get-WorkspaceKey $item.workspace) })
    if ($saved.Count -ne 1) { throw "Workspace registration could not be verified: $($item.workspace)" }
    if ($saved[0].Value.updates -ne $item.updates) { throw "Saved updates setting does not match: $($item.workspace)" }
    foreach ($name in $ranges.Keys) {
        if ($saved[0].Value.limits.$name -ne $item.limits.$name) { throw "Saved cap does not match: $($item.workspace) / $name" }
    }
}
$created
