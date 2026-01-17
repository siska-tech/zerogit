# Issue #003: テストフィクスチャの作成

## Phase
Phase 0: プロジェクトセットアップ

## 説明
テスト用のGitリポジトリフィクスチャを作成するスクリプトを用意する。

## タスク
- [x] `tests/fixtures/create_fixtures.sh` を作成
- [x] `simple/` リポジトリ（基本的なコミット）
- [x] `empty/` リポジトリ（コミットなし）
- [x] `branches/` リポジトリ（複数ブランチ）
- [x] CI用のセットアップ手順を文書化

## スクリプト内容
```bash
#!/bin/bash
set -e

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
```

## 受け入れ条件
- [x] スクリプトが正常に実行できる
- [x] 各フィクスチャが期待通りの構造を持つ
- [x] CIで自動実行される

## 依存
- #001

## ステータス
**完了** (2026-01-17)

## 備考
- Linux/macOS用の `create_fixtures.sh` に加えて、Windows用の `create_fixtures.ps1` も作成
- CI設定例を `tests/fixtures/README.md` に文書化
