param([switch]$Release, [switch]$Test)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path $PSScriptRoot -Parent
Push-Location $taskRoot
try {
    $taskCargo = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $taskCargo) {
        $taskCargoPath = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
        if (-not (Test-Path -LiteralPath $taskCargoPath)) { throw 'Install Rust from https://rustup.rs first.' }
    } else { $taskCargoPath = $taskCargo.Source }
    $taskToolchain = @()
    $taskPortable = Join-Path $taskRoot '.tools\w64devkit\bin'
    if (Test-Path -LiteralPath $taskPortable) {
        $env:PATH = $taskPortable + ';' + $env:PATH
        $taskRustup = Join-Path (Split-Path $taskCargoPath) 'rustup.exe'
        $taskRustc = & $taskRustup which --toolchain stable-x86_64-pc-windows-gnu rustc
        if ($LASTEXITCODE -ne 0) { throw 'Install the GNU toolchain with rustup toolchain install stable-x86_64-pc-windows-gnu.' }
        $taskSysroot = & $taskRustc --print sysroot
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $taskSysroot 'lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained\x86_64-w64-mingw32-gcc.exe'
        $env:RUSTFLAGS = '-C link-self-contained=yes'
        $taskToolchain = @('+stable-x86_64-pc-windows-gnu')
    }
    $taskArguments = @('build', '--locked')
    if ($Test) { $taskArguments = @('test', '--locked') }
    if ($Release) { $taskArguments += '--release' }
    & $taskCargoPath @taskToolchain @taskArguments
    if ($LASTEXITCODE -ne 0) { throw "Cargo failed with exit code $LASTEXITCODE" }
} finally { Pop-Location }
