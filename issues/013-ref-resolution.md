# Issue #013: 参照解決実装

## Phase
Phase 1: Refs Layer

## 説明
HEADおよびブランチ参照の解決機能を実装する。

## タスク
- [x] `src/refs/mod.rs` を作成
- [x] `src/refs/resolver.rs` を作成
- [x] `RefStore` 構造体を定義
- [x] `read_ref_file()` を実装
- [x] `resolve_recursive()` を実装
- [x] `head()` を実装
- [x] `branches()` を実装
- [x] ユニットテストを作成

## テストケース
- R-001〜R-005（テスト仕様書参照）

## 受け入れ条件
- [x] HEADを正しく解決できる
- [x] symbolic-refを再帰的に解決できる
- [x] ブランチ一覧を取得できる
- [x] テスト R-001〜R-005 がパス

## 依存
- #006
- #007

## 実装メモ
- `RefValue` enum: `Direct(Oid)` と `Symbolic(String)` の2種類
- `ResolvedRef` struct: 最終的に解決された参照名とOIDを保持
- `RefStore` struct: `.git` ディレクトリを基点に参照を解決
- 追加実装:
  - `current_branch()`: 現在のブランチ名を取得（detached HEADの場合はNone）
  - `tags()`: タグ一覧を取得
  - `resolve()`: 短縮名からの解決をサポート
- ネストされたブランチ名（例: `feature/my-feature`）もサポート
- 循環参照を検出するため最大10レベルの深さ制限あり
