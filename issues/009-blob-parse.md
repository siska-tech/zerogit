# Issue #009: Blobパース実装

## Phase
Phase 1: Objects Layer

## 説明
Blobオブジェクトのパースと公開APIを実装する。

## タスク
- [x] `src/objects/blob.rs` を作成
- [x] `Blob` 構造体を定義
- [x] `parse()` を実装
- [x] `content()`, `content_str()`, `size()`, `is_binary()` を実装
- [x] ユニットテストを作成

## テストケース
- B-001〜B-009（テスト仕様書参照）

## 受け入れ条件
- [x] Blobの内容を正しく取得できる
- [x] UTF-8変換が機能する
- [x] バイナリ判定が機能する
- [x] テスト B-001〜B-009 がパス

## 依存
- #007
- #008
