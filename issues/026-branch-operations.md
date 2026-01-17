# Issue #026: ブランチ操作実装

## Phase
Phase 2: 書き込み操作

## 説明
ブランチの作成・削除・切り替え機能を実装する。

## タスク
- [x] `Repository::create_branch()` を実装
- [x] `Repository::delete_branch()` を実装
- [x] `Repository::checkout()` を実装
- [x] 統合テストを作成

## テストケース
- W-006〜W-010（テスト仕様書参照）

## 受け入れ条件
- [x] ブランチを作成・削除できる
- [x] ブランチを切り替えられる
- [x] 現在のブランチは削除できない

## 依存
- #013
- #025

## 実装詳細

### Repository::create_branch(name, target)
- ブランチ名を検証（Gitのルールに従う）
- targetがNoneの場合はHEADを使用
- `.git/refs/heads/{name}` にターゲットOIDを書き込む
- ネストされたブランチ名（例: `feature/foo`）をサポート
- エラー: `InvalidRefName`, `RefAlreadyExists`

### Repository::delete_branch(name)
- カレントブランチは削除不可
- `.git/refs/heads/{name}` を削除
- 空の親ディレクトリをクリーンアップ
- エラー: `CannotDeleteCurrentBranch`, `RefNotFound`

### Repository::checkout(target)
- ワーキングツリーが汚れている場合はエラー
- ブランチ名 → シンボリックHEAD
- コミットOID → detached HEAD
- ワーキングツリーとインデックスを更新
- エラー: `DirtyWorkingTree`, `RefNotFound`

### テストファイル
- 単体テスト: `src/repository.rs` (W-006〜W-010)
- 統合テスト: `tests/branch_test.rs`
