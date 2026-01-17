# Issue #014: Head / Branch 型実装

## Phase
Phase 1: Refs Layer

## 説明
HEADとブランチを表す型を実装する。

## タスク
- [x] `src/refs/head.rs` を作成
- [x] `src/refs/branch.rs` を作成
- [x] `Head` enumを定義（Branch / Detached）
- [x] `Branch` 構造体を定義
- [x] 各メソッドを実装

## 受け入れ条件
- [x] detached HEAD状態を正しく表現できる
- [x] ブランチ情報を取得できる

## 依存
- #007
- #013

## 実装メモ

### Head enum (`src/refs/head.rs`)
- `Head::Branch { name, oid }`: ブランチを指すHEAD
- `Head::Detached { oid }`: 直接コミットを指すHEAD
- メソッド:
  - `branch()` / `detached()`: コンストラクタ
  - `oid()`: コミットOIDを取得
  - `branch_name()`: ブランチ名を取得（detachedの場合はNone）
  - `is_detached()` / `is_branch()`: 状態判定
  - `reference_name()`: 完全な参照名を取得

### Branch struct (`src/refs/branch.rs`)
- フィールド: `name`, `oid`, `is_current`
- メソッド:
  - `new()` / `current()`: コンストラクタ
  - `name()` / `oid()`: アクセサ
  - `is_current()`: 現在のブランチかどうか
  - `reference_name()`: `refs/heads/` プレフィックス付き名前
  - `short_oid()`: 7文字の短縮OID
  - `set_current()`: 現在ブランチフラグを変更
- `Display` トレイト: `* main` または `  develop` 形式で表示

### BranchList struct
- ブランチのコレクション
- メソッド:
  - `current()`: 現在のブランチを取得
  - `find()`: 名前で検索
  - `sort_by_name()`: 名前でソート
  - `IntoIterator` 実装

### テスト結果
- head.rs: 6テスト
- branch.rs: 10テスト
- 全133テスト成功
