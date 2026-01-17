# Issue #006: ファイルシステムユーティリティ実装

## Phase
Phase 1: Infrastructure Layer

## 説明
ファイル読み書きとディレクトリ走査のユーティリティ関数を実装する。

## タスク
- [x] `src/infra/fs.rs` を作成
- [x] `read_file()` 関数を実装
- [x] `write_file_atomic()` 関数を実装（Phase 2用だが先に実装）
- [x] `list_working_tree()` 関数を実装
- [x] パストラバーサル防止を実装

## 受け入れ条件
- [x] ファイルの読み書きができる
- [x] `.git` ディレクトリが除外される
- [x] アトミック書き込みが機能する

## 依存
- #001
- #002
