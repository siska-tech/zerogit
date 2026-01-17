# Issue #024: Repository::add()実装

## Phase
Phase 2: 書き込み操作

## 説明
ファイルをステージングエリアに追加する機能を実装する。

## タスク
- [x] `Repository::add()` を実装
- [x] `Repository::add_all()` を実装
- [x] `Repository::reset()` を実装
- [x] 統合テストを作成

## テストケース
- W-001〜W-003（テスト仕様書参照）

## 受け入れ条件
- [x] ファイルをステージできる
- [x] ステージを解除できる
- [x] `git status` で正しく反映される

## 依存
- #022
- #023

## 実装概要

### Repository::add(path)
- ファイルをステージングエリアに追加
- Blobオブジェクトを作成してオブジェクトストアに保存
- インデックスを更新

### Repository::add_all()
- 全ての変更（新規、変更、削除）をステージ
- `git add -A` と同等

### Repository::reset(path)
- 指定されたパス（またはすべて）のステージを解除
- HEADの状態にインデックスを戻す

### Index操作メソッド追加
- `Index::empty(version)` - 空のインデックスを作成
- `Index::add(entry)` - エントリを追加/更新
- `Index::remove(path)` - エントリを削除
- `Index::clear()` - 全エントリを削除

### 統合テスト（tests/staging_test.rs）
- W-001: add()でファイルをステージ
- W-002: add_all()で全変更をステージ
- W-003: reset()でステージを解除
