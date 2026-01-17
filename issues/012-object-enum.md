# Issue #012: Object enum実装

## Phase
Phase 1: Objects Layer

## 説明
各オブジェクト型を統合するObject enumを実装する。

## タスク
- [x] `Object` enumを `src/objects/mod.rs` に定義
- [x] `kind()` メソッドを実装
- [x] `as_blob()`, `as_tree()`, `as_commit()` を実装
- [x] `into_blob()`, `into_tree()`, `into_commit()` を実装

## 受け入れ条件
- [x] 任意のオブジェクトをObject enumで扱える
- [x] 型変換メソッドが正しく動作する

## 依存
- #009
- #010
- #011

## 実装メモ
- `Object` enumは `Blob`, `Tree`, `Commit` の3つのバリアントを持つ
- `From` トレイトを実装して各オブジェクト型からの変換をサポート
- テストケース O-001〜O-011 で動作を検証済み
