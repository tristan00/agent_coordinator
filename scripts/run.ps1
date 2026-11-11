param([int]$Port = 4242, [string]$DataDir = '')
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path $PSScriptRoot -Parent
if (-not $DataDir) { $DataDir = Join-Path $taskRoot 'data' }
$taskExecutable = Join-Path $taskRoot 'target\release\relay-coordinator.exe'
if (-not (Test-Path -LiteralPath $taskExecutable)) { $taskExecutable = Join-Path $taskRoot 'target\debug\relay-coordinator.exe' }
if (-not (Test-Path -LiteralPath $taskExecutable)) { & (Join-Path $PSScriptRoot 'build.ps1'); $taskExecutable = Join-Path $taskRoot 'target\debug\relay-coordinator.exe' }
$taskArguments = @('--port', $Port, '--data-dir', $DataDir)
& $taskExecutable @taskArguments
