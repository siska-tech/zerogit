# Issue #005: zlib解凍ラッパー実装

## Phase
Phase 1: Infrastructure Layer

## 説明
miniz_oxideを使用したzlib解凍機能のラッパーを実装する。

## タスク
- [x] `src/infra/compression.rs` を作成
- [x] `decompress()` 関数を実装
- [x] zlibヘッダー検証を実装
- [x] エラーハンドリングを実装
- [x] ユニットテストを作成

## テストケース
- C-001: 正常な解凍
- C-002: 破損データのエラー
- C-003: 空データのエラー
- C-004: 切り詰めデータのエラー

## 受け入れ条件
- [x] 有効なzlibデータを正しく解凍できる
- [x] 不正なデータで `Error::DecompressionFailed` を返す
- [x] テスト C-001〜C-004 がパス

## 依存
- #001
- #002
