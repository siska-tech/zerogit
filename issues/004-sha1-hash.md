# Issue #004: SHA-1ハッシュ実装

## Phase
Phase 1: Infrastructure Layer

## 説明
RFC 3174準拠のSHA-1ハッシュ関数を自前実装する。

## タスク
- [x] `src/infra/mod.rs` を作成
- [x] `src/infra/hash.rs` を作成
- [x] `Sha1State` 構造体を実装
- [x] `process_block()` 関数を実装
- [x] `sha1()` 公開関数を実装
- [x] `hash_object()` 公開関数を実装
- [x] ユニットテストを作成

## テストケース
- H-001: 空データのハッシュ
- H-002: "hello world" のハッシュ
- H-003: バイナリデータのハッシュ
- H-004: 大きなデータのハッシュ
- H-005: Gitオブジェクト形式のハッシュ

## 受け入れ条件
- [x] 既知のテストベクターで正しいハッシュ値が得られる
- [x] `git hash-object` と同じ結果が得られる
- [x] テスト H-001〜H-005 がパス

## 依存
- #001
- #002

## ステータス
**完了** (2026-01-17)

## 実装詳細
- RFC 3174準拠のSHA-1実装
- 9テスト全てパス（H-001〜H-005 + RFC標準テストベクター）
- `git hash-object --stdin`と同一結果を確認
