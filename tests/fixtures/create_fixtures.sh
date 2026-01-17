#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "Creating test fixtures in $SCRIPT_DIR"

# simple: 基本リポジトリ
rm -rf simple
mkdir -p simple && cd simple
git init
git config user.email "test@example.com"
git config user.name "Test User"
echo "Hello" > README.md
git add README.md
git commit -m "Initial commit"
echo "World" >> README.md
git add README.md
git commit -m "Second commit"
cd ..

# empty: 空リポジトリ
rm -rf empty
mkdir -p empty && cd empty
git init
git config user.email "test@example.com"
git config user.name "Test User"
cd ..

# branches: 複数ブランチ
rm -rf branches
mkdir -p branches && cd branches
git init
git config user.email "test@example.com"
git config user.name "Test User"
echo "main" > file.txt
git add file.txt
git commit -m "Main commit"
git checkout -b feature
echo "feature" > feature.txt
git add feature.txt
git commit -m "Feature commit"
git checkout main
cd ..

echo "Fixtures created successfully"
