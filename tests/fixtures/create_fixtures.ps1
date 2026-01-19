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

# remotes: リモートブランチのあるリポジトリ
if (Test-Path "remotes") { Remove-Item -Recurse -Force "remotes" }
New-Item -ItemType Directory -Path "remotes" | Out-Null
Set-Location "remotes"
git init
git config user.email "test@example.com"
git config user.name "Test User"
Set-Content -Path "file.txt" -Value "main"
git add file.txt
git commit -m "Initial commit"
# 疑似的にリモートブランチを作成
New-Item -ItemType Directory -Path ".git/refs/remotes/origin" -Force | Out-Null
$mainOid = git rev-parse HEAD
Set-Content -Path ".git/refs/remotes/origin/main" -Value $mainOid -NoNewline
Set-Content -Path ".git/refs/remotes/origin/develop" -Value $mainOid -NoNewline
New-Item -ItemType Directory -Path ".git/refs/remotes/origin/feature" -Force | Out-Null
Set-Content -Path ".git/refs/remotes/origin/feature/xyz" -Value $mainOid -NoNewline
New-Item -ItemType Directory -Path ".git/refs/remotes/upstream" -Force | Out-Null
Set-Content -Path ".git/refs/remotes/upstream/main" -Value $mainOid -NoNewline
Set-Location ..

# tags: タグのあるリポジトリ
if (Test-Path "tags") { Remove-Item -Recurse -Force "tags" }
New-Item -ItemType Directory -Path "tags" | Out-Null
Set-Location "tags"
git init
git config user.email "test@example.com"
git config user.name "Test User"
Set-Content -Path "file.txt" -Value "v1"
git add file.txt
git commit -m "Version 1"
git tag v1.0.0                          # 軽量タグ
git tag -a v1.0.1 -m "Annotated tag"    # 注釈付きタグ
Set-Location ..

# diff: 差分テスト用リポジトリ
if (Test-Path "diff") { Remove-Item -Recurse -Force "diff" }
New-Item -ItemType Directory -Path "diff" | Out-Null
Set-Location "diff"
git init
git config user.email "test@example.com"
git config user.name "Test User"
# 初期コミット
Set-Content -Path "file1.txt" -Value "initial"
Set-Content -Path "file2.txt" -Value "to-delete"
New-Item -ItemType Directory -Path "src" -Force | Out-Null
Set-Content -Path "src/main.rs" -Value "fn main() {}"
git add .
git commit -m "Initial commit"
# 変更コミット
Set-Content -Path "file1.txt" -Value "modified"          # Modified
Remove-Item "file2.txt"                                   # Deleted
Set-Content -Path "file3.txt" -Value "new file"          # Added
Set-Content -Path "src/main.rs" -Value 'fn main() { println!("hello"); }'  # Modified
git add .
git commit -m "Various changes"
Set-Location ..

# rename: リネームテスト用リポジトリ
if (Test-Path "rename") { Remove-Item -Recurse -Force "rename" }
New-Item -ItemType Directory -Path "rename" | Out-Null
Set-Location "rename"
git init
git config user.email "test@example.com"
git config user.name "Test User"
# 初期コミット
Set-Content -Path "old_name.txt" -Value "content"
Set-Content -Path "keep.txt" -Value "unchanged"
git add .
git commit -m "Initial commit"
# リネームコミット
git mv old_name.txt new_name.txt
git commit -m "Rename file"
Set-Location ..

# merge: マージコミットテスト用リポジトリ
if (Test-Path "merge") { Remove-Item -Recurse -Force "merge" }
New-Item -ItemType Directory -Path "merge" | Out-Null
Set-Location "merge"
git init
git config user.email "test@example.com"
git config user.name "Test User"
# 初期コミット
Set-Content -Path "main.txt" -Value "main"
git add .
git commit -m "Initial commit"
# featureブランチ
git checkout -b feature
Set-Content -Path "feature.txt" -Value "feature"
git add feature.txt
git commit -m "Add feature"
# mainブランチに戻って別の変更
git checkout master
Set-Content -Path "main2.txt" -Value "main2"
git add main2.txt
git commit -m "Add main2"
# マージ
git merge feature --no-ff -m "Merge feature"
Set-Location ..

Write-Host "Fixtures created successfully"
