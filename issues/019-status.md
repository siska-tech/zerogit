# Issue #019: Status実装

## Phase
Phase 1: Repository Layer

## 説明
ワーキングツリーの状態取得機能を実装する。

## タスク
- [x] `src/status.rs` を作成
- [x] `StatusEntry`, `FileStatus` を定義
- [x] `flatten_tree()` を実装
- [x] `file_modified()` を実装
- [x] `compute_status()` を実装
- [x] `Repository::status()` を実装
- [x] 統合テストを作成

## テストケース
- RP-020〜RP-024（テスト仕様書参照）

## 受け入れ条件
- [x] Untracked/Modified/Deleted/Stagedを正しく検出
- [x] HEAD/Index/Working treeの三方比較が機能する
- [x] テスト RP-020〜RP-024 がパス

## 依存
- #015
- #017
