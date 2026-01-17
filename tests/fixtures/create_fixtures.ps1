# PowerShell script to create test fixtures
$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $ScriptDir

Write-Host "Creating test fixtures in $ScriptDir"

# simple: 基本リポジトリ
if (Test-Path "simple") { Remove-Item -Recurse -Force "simple" }
New-Item -ItemType Directory -Path "simple" | Out-Null
Set-Location "simple"
git init
git config user.email "test@example.com"
git config user.name "Test User"
Set-Content -Path "README.md" -Value "Hello" -NoNewline
Add-Content -Path "README.md" -Value ""
git add README.md
git commit -m "Initial commit"
Set-Content -Path "README.md" -Value "Hello" -NoNewline
Add-Content -Path "README.md" -Value ""
Add-Content -Path "README.md" -Value "World"
git add README.md
git commit -m "Second commit"
Set-Location ..

# empty: 空リポジトリ
if (Test-Path "empty") { Remove-Item -Recurse -Force "empty" }
New-Item -ItemType Directory -Path "empty" | Out-Null
Set-Location "empty"
git init
git config user.email "test@example.com"
git config user.name "Test User"
Set-Location ..

# branches: 複数ブランチ
if (Test-Path "branches") { Remove-Item -Recurse -Force "branches" }
New-Item -ItemType Directory -Path "branches" | Out-Null
Set-Location "branches"
git init
git config user.email "test@example.com"
git config user.name "Test User"
Set-Content -Path "file.txt" -Value "main"
git add file.txt
git commit -m "Main commit"
git checkout -b feature
Set-Content -Path "feature.txt" -Value "feature"
git add feature.txt
git commit -m "Feature commit"
git checkout master
Set-Location ..

Write-Host "Fixtures created successfully"
