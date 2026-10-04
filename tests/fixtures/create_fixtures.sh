#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "Creating test fixtures in $SCRIPT_DIR"

# Ignore user/system config (default branch, autocrlf, signing, hooks) so
# fixtures are identical on every machine and OS.
export GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_GLOBAL=/dev/null
# Background auto-maintenance would repack fixtures nondeterministically.
export GIT_CONFIG_COUNT=2
export GIT_CONFIG_KEY_0=maintenance.auto GIT_CONFIG_VALUE_0=false
export GIT_CONFIG_KEY_1=gc.auto GIT_CONFIG_VALUE_1=0
export GIT_AUTHOR_NAME="Test User" GIT_AUTHOR_EMAIL="test@example.com"
export GIT_COMMITTER_NAME="Test User" GIT_COMMITTER_EMAIL="test@example.com"
export GIT_AUTHOR_DATE="2024-01-01T00:00:00Z" GIT_COMMITTER_DATE="2024-01-01T00:00:00Z"

# Creates an empty repository on branch main and enters it.
new_repo() {
    rm -rf "$1"
    mkdir -p "$1"
    cd "$1"
    git init -q
    git symbolic-ref HEAD refs/heads/main
    git config core.autocrlf false
    git config user.email "test@example.com"
    git config user.name "Test User"
}

commit() {
    git add -A
    git commit -q -m "$1"
}

# simple: 基本リポジトリ（2コミット）
new_repo simple
echo "Hello" > README.md
commit "Initial commit"
echo "World" >> README.md
commit "Second commit"
cd ..

# empty: 空リポジトリ（コミットなし）
new_repo empty
cd ..

# branches: 複数ブランチ
new_repo branches
echo "main" > file.txt
commit "Main commit"
git checkout -q -b feature
echo "feature" > feature.txt
commit "Feature commit"
git checkout -q main
cd ..

# remotes: リモート追跡ブランチ（疑似）
new_repo remotes
echo "main" > file.txt
commit "Initial commit"
oid="$(git rev-parse HEAD)"
git update-ref refs/remotes/origin/main "$oid"
git update-ref refs/remotes/origin/develop "$oid"
git update-ref refs/remotes/origin/feature/xyz "$oid"
git update-ref refs/remotes/upstream/main "$oid"
cd ..

# tags: 軽量タグと注釈付きタグ
new_repo tags
echo "v1" > file.txt
commit "Version 1"
git tag v1.0.0
git tag -a v1.0.1 -m "Annotated tag"
cd ..

# diff: 追加・削除・変更（ネスト含む）
new_repo diff
echo "initial" > file1.txt
echo "to-delete" > file2.txt
mkdir -p src
echo "fn main() {}" > src/main.rs
commit "Initial commit"
echo "modified" > file1.txt
rm file2.txt
echo "new file" > file3.txt
echo 'fn main() { println!("hello"); }' > src/main.rs
commit "Various changes"
cd ..

# rename: 完全一致リネーム
new_repo rename
echo "content" > old_name.txt
echo "unchanged" > keep.txt
commit "Initial commit"
git mv old_name.txt new_name.txt
commit "Rename file"
cd ..

# merge: --no-ffマージコミット
new_repo merge
echo "main" > main.txt
commit "Initial commit"
git checkout -q -b feature
echo "feature" > feature.txt
commit "Add feature"
git checkout -q main
echo "main2" > main2.txt
commit "Add main2"
git merge -q --no-ff -m "Merge feature" feature
cd ..

echo "Fixtures created successfully"
