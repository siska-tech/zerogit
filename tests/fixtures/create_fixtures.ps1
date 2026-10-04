# PowerShell wrapper: fixtures are defined once in create_fixtures.sh.
# Uses the bash bundled with Git for Windows (not WSL's bash.exe).
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$GitRoot = Split-Path (Split-Path (Get-Command git).Source)
$Bash = Join-Path $GitRoot "bin\bash.exe"
if (-not (Test-Path $Bash)) { throw "Git Bash not found at $Bash" }
& $Bash (Join-Path $ScriptDir "create_fixtures.sh")
if ($LASTEXITCODE -ne 0) { throw "create_fixtures.sh failed with exit code $LASTEXITCODE" }
