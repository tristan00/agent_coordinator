param(
    [Parameter(Mandatory = $true)][string[]]$Workspace,
    [string]$Planner = '',
    [string]$Worker = '',
    [string]$ProjectPlanner = '',
    [string]$WorkerProtocol = '',
    [string]$CoordinatorUrl = 'http://127.0.0.1:4242',
    [Nullable[int]]$Seed,
    [ValidateSet('', 'poll', 'events')][string]$Updates = ''
)

$ErrorActionPreference = 'Stop'
$baseUrl = $CoordinatorUrl.TrimEnd('/')
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
$catalog = Invoke-RestMethod -Uri "$baseUrl/api/prompt-candidates" -TimeoutSec 15
$eligible = @($catalog.agents.agents | Where-Object { $_.allowed })
$agents = @($eligible | ForEach-Object { $_.id })
$sampled = @($eligible | Where-Object { $_.agent.provider -ne 'local' } | ForEach-Object { $_.id })
$planners = @($eligible | Where-Object { $_.planner -and $_.agent.provider -ne 'local' } | ForEach-Object { $_.id })
if ($Planner -and $Planner -notin @($eligible | Where-Object { $_.planner } | ForEach-Object { $_.id })) { throw "Agent cannot plan: $Planner" }
if (!$agents.Count) { throw 'Coordinator returned no eligible workspace agents' }
if ((!$Planner -or !$Worker) -and !$sampled.Count) { throw 'No hosted agents are allowed for this launcher; pass -Planner and -Worker explicitly' }
if ($Planner -and $Planner -notin $agents) { throw "Ineligible planner: $Planner" }
if ($Worker -and $Worker -notin $agents) { throw "Ineligible worker: $Worker" }
$planners = @($catalog.project_planner | ForEach-Object { $_.candidate })
$workers = @($catalog.worker_protocol | ForEach-Object { $_.candidate })
if (!$planners.Count -or !$workers.Count) { throw "Both prompt_candidate/project_planner and prompt_candidate/worker_protocol need a valid nonempty prompt of at most 200 words: $($catalog.directory)" }
if ($ProjectPlanner -and $ProjectPlanner -notin $planners) { throw "Invalid project_planner candidate: $ProjectPlanner" }
if ($WorkerProtocol -and $WorkerProtocol -notin $workers) { throw "Invalid worker_protocol candidate: $WorkerProtocol" }
$state = Invoke-RestMethod -Uri "$baseUrl/api/state" -TimeoutSec 15
$known = @{}
foreach ($property in $state.config.workspaces.PSObject.Properties) { $known[(Get-WorkspaceKey $property.Name)] = $true }
$paths = foreach ($item in $Workspace) {
    if ([string]::IsNullOrWhiteSpace($item)) { throw 'Workspace paths must not be empty' }
    $path = [IO.Path]::GetFullPath($item)
    if (Test-Path -LiteralPath $path) { throw "Workspace already exists: $path" }
    $key = Get-WorkspaceKey $path
    if ($known.ContainsKey($key)) { throw "Workspace is already configured or repeated: $path" }
    $known[$key] = $true
    $path
}
$created = foreach ($path in $paths) {
    $limits = [ordered]@{}
    foreach ($entry in $ranges.GetEnumerator()) { $limits[$entry.Key] = $random.Next($entry.Value[0], $entry.Value[1] + 1) }
    $mode = if ($Updates) { $Updates } else { @('poll', 'events')[$random.Next(0, 2)] }
    $plannerPrompt = if ($ProjectPlanner) { $ProjectPlanner } else { $planners[$random.Next(0, $planners.Count)] }
    $workerPrompt = if ($WorkerProtocol) { $WorkerProtocol } else { $workers[$random.Next(0, $workers.Count)] }
    $selectedPlanner = if ($Planner) { $Planner } else { $planners[$random.Next(0, $planners.Count)] }
    $selectedWorker = if ($Worker) { $Worker } else { $sampled[$random.Next(0, $sampled.Count)] }
    $body = @{ path = $path; project_planner = $plannerPrompt; worker_protocol = $workerPrompt;
        settings = @{ limits = $limits; updates = $mode; planner = $selectedPlanner; worker = $selectedWorker } }
    Invoke-RestMethod -Uri "$baseUrl/api/workspaces" -Method Post -ContentType 'application/json' -Body ($body | ConvertTo-Json -Depth 10) -TimeoutSec 15
}
$verified = Invoke-RestMethod -Uri "$baseUrl/api/state" -TimeoutSec 15
foreach ($item in $created) {
    $saved = @($verified.config.workspaces.PSObject.Properties | Where-Object { (Get-WorkspaceKey $_.Name) -eq (Get-WorkspaceKey $item.workspace) })
    if ($saved.Count -ne 1) { throw "Workspace registration could not be verified: $($item.workspace)" }
    if ($saved[0].Value.planner -ne $item.settings.planner -or $saved[0].Value.worker -ne $item.settings.worker) { throw "Saved agent selection does not match: $($item.workspace)" }
    foreach ($role in @('project_planner', 'worker_protocol')) {
        if ($saved[0].Value.prompts.$role.sha256 -ne $item.settings.prompts.$role.sha256) { throw "Saved prompt does not match: $($item.workspace) / $role" }
    }
}
$created
