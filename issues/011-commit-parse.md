# Issue #011: Commitパース実装

## Phase
Phase 1: Objects Layer

## 説明
Commitオブジェクトのパースと公開APIを実装する。

## タスク
- [x] `src/objects/commit.rs` を作成
- [x] `Commit`, `Signature` 構造体を定義
- [x] `parse()` を実装
- [x] `parse_signature()` を実装
- [x] 各アクセサメソッドを実装
- [x] `summary()` を実装
- [x] ユニットテストを作成

## テストケース
- CM-001〜CM-009（テスト仕様書参照）

## 受け入れ条件
- [x] Commitの全フィールドをパースできる
- [x] 親コミットを正しく取得できる
- [x] Signatureのタイムゾーンを正しくパースできる
- [x] テスト CM-001〜CM-009 がパス

## 依存
- #007
- #008
