# Issue #008: オブジェクトストア実装

## Phase
Phase 1: Objects Layer

## 説明
Looseオブジェクトの読み取り機能を実装する。

## タスク
- [x] `src/objects/store.rs` を作成
- [x] `LooseObjectStore` 構造体を定義
- [x] `oid_to_path()` を実装
- [x] `read_raw()` を実装
- [x] `parse_raw_object()` を実装
- [x] `read()` 公開メソッドを実装
- [x] `exists()` を実装
- [x] `find_objects_by_prefix()` を実装（短縮SHA対応）

## 受け入れ条件
- [x] `.git/objects/` からオブジェクトを読み取れる
- [x] zlib解凍とヘッダーパースが正しく動作する
- [x] 短縮SHAでオブジェクトを検索できる

## 依存
- #005
- #006
- #007
