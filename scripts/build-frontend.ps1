param([Parameter(Mandatory=$true)][string]$Workspace,[Parameter(Mandatory=$true)][string]$Store,[Parameter(Mandatory=$true)][string]$Cache,[Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference='Stop'
$project=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
foreach($directory in @($Workspace,$Store,$Cache)) {
    $absolute=[IO.Path]::GetFullPath($directory)
    if($absolute.Equals($project,[StringComparison]::OrdinalIgnoreCase) -or $absolute.StartsWith($project+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) { throw 'Build data must be outside the source directory' }
}
New-Item -ItemType Directory -Force $Workspace,$OutputDirectory | Out-Null
Copy-Item (Join-Path $project 'frontend/package.json'),(Join-Path $project 'frontend/pnpm-lock.yaml'),(Join-Path $project 'frontend/index.html'),(Join-Path $project 'frontend/vite.config.js') $Workspace -Force
New-Item -ItemType Directory -Force (Join-Path $Workspace 'src') | Out-Null
Copy-Item (Join-Path $project 'frontend/src/*') (Join-Path $Workspace 'src') -Force
pnpm --dir $Workspace --store-dir $Store --cache-dir $Cache install --offline --frozen-lockfile --reporter append-only
if($LASTEXITCODE -ne 0) { throw 'Frozen frontend installation failed' }
pnpm --dir $Workspace exec vite build
if($LASTEXITCODE -ne 0) { throw 'Frontend build failed' }
Copy-Item (Join-Path $Workspace 'dist/index.html'),(Join-Path $Workspace 'dist/app.js'),(Join-Path $Workspace 'dist/style.css') $OutputDirectory -Force
