# Issue #010: Treeパース実装

## Phase
Phase 1: Objects Layer

## 説明
Treeオブジェクトのパースと公開APIを実装する。

## タスク
- [x] `src/objects/tree.rs` を作成
- [x] `Tree`, `TreeEntry` 構造体を定義
- [x] `FileMode` enumを定義
- [x] `parse()` を実装
- [x] `entries()`, `get()`, `iter()` を実装
- [x] `TreeEntry` のメソッドを実装
- [x] ユニットテストを作成

## テストケース
- T-001〜T-009（テスト仕様書参照）

## 受け入れ条件
- [x] Treeエントリを正しくパースできる
- [x] 名前でエントリを検索できる
- [x] FileModeを正しく判定できる
- [x] テスト T-001〜T-009 がパス

## 依存
- #007
- #008
