# zerogit 実装タスク一覧

## 概要

本ドキュメントは、zerogitの実装を進めるためのタスク（Issue）一覧です。
各タスクは依存関係を考慮した順序で並んでいます。

---

## Phase 0: プロジェクトセットアップ

### Issue #001: プロジェクト初期化 ✅

**説明:**
Cargoプロジェクトの作成と基本構成のセットアップ。

**タスク:**
- [x] `cargo new zerogit --lib` でプロジェクト作成
- [x] `Cargo.toml` の設定（メタデータ、依存関係）
- [x] ディレクトリ構造の作成
- [x] `.gitignore` の作成
- [x] LICENSE-MIT, LICENSE-APACHE の作成
- [x] README.md の配置

**Cargo.toml:**
```toml
[package]
name = "zerogit"
version = "0.1.0"
edition = "2021"
rust-version = "1.70"
description = "A lightweight, pure Rust Git client library"
license = "MIT OR Apache-2.0"
repository = "https://github.com/siska-tech/zerogit"
keywords = ["git", "vcs", "pure-rust"]
categories = ["development-tools"]

[dependencies]
miniz_oxide = "0.8"

[dev-dependencies]
tempfile = "3"
```

**ディレクトリ構造:**
```
zerogit/
├── Cargo.toml
├── LICENSE-MIT
├── LICENSE-APACHE
├── README.md
├── src/
│   ├── lib.rs
│   ├── error.rs
│   ├── repository.rs
│   ├── objects/
│   │   └── mod.rs
│   ├── refs/
│   │   └── mod.rs
│   ├── index/
│   │   └── mod.rs
│   └── infra/
│       └── mod.rs
└── tests/
    ├── fixtures/
    │   └── create_fixtures.sh
    └── integration/
```

**受け入れ条件:**
- [x] `cargo build` が成功する
- [x] `cargo test` が実行できる（テストは空でOK）
- [x] `cargo clippy` で警告なし

**依存:** なし

---

### Issue #002: エラー型の定義 ✅

**説明:**
プロジェクト全体で使用するエラー型を定義する。

**タスク:**
- [x] `src/error.rs` を作成
- [x] `Error` enum を定義
- [x] `Display`, `std::error::Error` トレイトを実装
- [x] `From<std::io::Error>` を実装
- [x] `Result<T>` 型エイリアスを定義

**実装内容:**
```rust
// src/error.rs
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    NotARepository(std::path::PathBuf),
    ObjectNotFound(Oid),
    RefNotFound(String),
    PathNotFound(std::path::PathBuf),
    InvalidOid(String),
    InvalidRefName(String),
    InvalidObject { oid: Oid, reason: String },
    InvalidIndex { version: u32, reason: String },
    TypeMismatch { expected: &'static str, actual: &'static str },
    InvalidUtf8,
    DecompressionFailed,
    // Phase 2
    RefAlreadyExists(String),
    CannotDeleteCurrentBranch,
    EmptyCommit,
    DirtyWorkingTree,
    ConfigNotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;
```

**受け入れ条件:**
- [x] すべてのエラーバリアントが定義されている
- [x] `Display` で人間可読なメッセージが出力される
- [x] テスト E-001〜E-004 がパス

**依存:** #001

---

### Issue #003: テストフィクスチャの作成 ✅

**説明:**
テスト用のGitリポジトリフィクスチャを作成するスクリプトを用意する。

**タスク:**
- [x] `tests/fixtures/create_fixtures.sh` を作成
- [x] `simple/` リポジトリ（基本的なコミット）
- [x] `empty/` リポジトリ（コミットなし）
- [x] `branches/` リポジトリ（複数ブランチ）
- [x] CI用のセットアップ手順を文書化

**スクリプト内容:**
```bash
#!/bin/bash
set -e

# simple: 基本リポジトリ
rm -rf simple
mkdir -p simple && cd simple
git init
git config user.email "test@example.com"
git config user.name "Test User"
echo "Hello" > README.md
git add README.md
git commit -m "Initial commit"
echo "World" >> README.md
git add README.md
git commit -m "Second commit"
cd ..

# empty: 空リポジトリ
rm -rf empty
mkdir -p empty && cd empty
git init
git config user.email "test@example.com"
git config user.name "Test User"
cd ..

# branches: 複数ブランチ
rm -rf branches
mkdir -p branches && cd branches
git init
git config user.email "test@example.com"
git config user.name "Test User"
echo "main" > file.txt
git add file.txt
git commit -m "Main commit"
git checkout -b feature
echo "feature" > feature.txt
git add feature.txt
git commit -m "Feature commit"
git checkout main
cd ..

echo "Fixtures created successfully"
```

**受け入れ条件:**
- [x] スクリプトが正常に実行できる
- [x] 各フィクスチャが期待通りの構造を持つ
- [x] CIで自動実行される

**依存:** #001

---

## Phase 1: Infrastructure Layer

### Issue #004: SHA-1ハッシュ実装 ✅

**説明:**
RFC 3174準拠のSHA-1ハッシュ関数を自前実装する。

**タスク:**
- [x] `src/infra/mod.rs` を作成
- [x] `src/infra/hash.rs` を作成
- [x] `Sha1State` 構造体を実装
- [x] `process_block()` 関数を実装
- [x] `sha1()` 公開関数を実装
- [x] `hash_object()` 公開関数を実装
- [x] ユニットテストを作成

**テストケース:**
- H-001: 空データのハッシュ
- H-002: "hello world" のハッシュ
- H-003: バイナリデータのハッシュ
- H-004: 大きなデータのハッシュ
- H-005: Gitオブジェクト形式のハッシュ

**受け入れ条件:**
- [x] 既知のテストベクターで正しいハッシュ値が得られる
- [x] `git hash-object` と同じ結果が得られる
- [x] テスト H-001〜H-005 がパス

**依存:** #001, #002

---

### Issue #005: zlib解凍ラッパー実装 ✅

**説明:**
miniz_oxideを使用したzlib解凍機能のラッパーを実装する。

**タスク:**
- [x] `src/infra/compression.rs` を作成
- [x] `decompress()` 関数を実装
- [x] zlibヘッダー検証を実装
- [x] エラーハンドリングを実装
- [x] ユニットテストを作成

**テストケース:**
- C-001: 正常な解凍
- C-002: 破損データのエラー
- C-003: 空データのエラー
- C-004: 切り詰めデータのエラー

**受け入れ条件:**
- [x] 有効なzlibデータを正しく解凍できる
- [x] 不正なデータで `Error::DecompressionFailed` を返す
- [x] テスト C-001〜C-004 がパス

**依存:** #001, #002

---

### Issue #006: ファイルシステムユーティリティ実装 ✅

**説明:**
ファイル読み書きとディレクトリ走査のユーティリティ関数を実装する。

**タスク:**
- [x] `src/infra/fs.rs` を作成
- [x] `read_file()` 関数を実装
- [x] `write_file_atomic()` 関数を実装（Phase 2用だが先に実装）
- [x] `list_working_tree()` 関数を実装
- [x] パストラバーサル防止を実装

**受け入れ条件:**
- [x] ファイルの読み書きができる
- [x] `.git` ディレクトリが除外される
- [x] アトミック書き込みが機能する

**依存:** #001, #002

---

## Phase 1: Objects Layer

### Issue #007: Oid（オブジェクトID）実装 ✅

**説明:**
SHA-1ハッシュを表すOid型を実装する。

**タスク:**
- [x] `src/objects/mod.rs` を作成
- [x] `src/objects/oid.rs` を作成
- [x] `Oid` 構造体を定義
- [x] `from_hex()`, `from_bytes()` を実装
- [x] `to_hex()`, `short()`, `as_bytes()` を実装
- [x] `Display`, `Debug`, `FromStr` トレイトを実装
- [x] ユニットテストを作成

**テストケース:**
- O-001〜O-009（テスト仕様書参照）

**受け入れ条件:**
- [x] 40文字の16進数文字列から変換できる
- [x] 大文字は小文字に正規化される
- [x] 不正な入力でエラーを返す
- [x] テスト O-001〜O-009 がパス

**依存:** #002, #004

---

### Issue #008: オブジェクトストア実装 ✅

**説明:**
Looseオブジェクトの読み取り機能を実装する。

**タスク:**
- [x] `src/objects/store.rs` を作成
- [x] `LooseObjectStore` 構造体を定義
- [x] `oid_to_path()` を実装
- [x] `read_raw()` を実装
- [x] `parse_raw_object()` を実装
- [x] `read()` 公開メソッドを実装
- [x] `exists()` を実装
- [x] `find_objects_by_prefix()` を実装（短縮SHA対応）

**受け入れ条件:**
- [x] `.git/objects/` からオブジェクトを読み取れる
- [x] zlib解凍とヘッダーパースが正しく動作する
- [x] 短縮SHAでオブジェクトを検索できる

**依存:** #005, #006, #007

---

### Issue #009: Blobパース実装 ✅

**説明:**
Blobオブジェクトのパースと公開APIを実装する。

**タスク:**
- [x] `src/objects/blob.rs` を作成
- [x] `Blob` 構造体を定義
- [x] `parse()` を実装
- [x] `content()`, `content_str()`, `size()`, `is_binary()` を実装
- [x] ユニットテストを作成

**テストケース:**
- B-001〜B-009（テスト仕様書参照）

**受け入れ条件:**
- [x] Blobの内容を正しく取得できる
- [x] UTF-8変換が機能する
- [x] バイナリ判定が機能する
- [x] テスト B-001〜B-009 がパス

**依存:** #007, #008

---

### Issue #010: Treeパース実装 ✅

**説明:**
Treeオブジェクトのパースと公開APIを実装する。

**タスク:**
- [x] `src/objects/tree.rs` を作成
- [x] `Tree`, `TreeEntry` 構造体を定義
- [x] `FileMode` enumを定義
- [x] `parse()` を実装
- [x] `entries()`, `get()`, `iter()` を実装
- [x] `TreeEntry` のメソッドを実装
- [x] ユニットテストを作成

**テストケース:**
- T-001〜T-009（テスト仕様書参照）

**受け入れ条件:**
- [x] Treeエントリを正しくパースできる
- [x] 名前でエントリを検索できる
- [x] FileModeを正しく判定できる
- [x] テスト T-001〜T-009 がパス

**依存:** #007, #008

---

### Issue #011: Commitパース実装 ✅

**説明:**
Commitオブジェクトのパースと公開APIを実装する。

**タスク:**
- [x] `src/objects/commit.rs` を作成
- [x] `Commit`, `Signature` 構造体を定義
- [x] `parse()` を実装
- [x] `parse_signature()` を実装
- [x] 各アクセサメソッドを実装
- [x] `summary()` を実装
- [x] ユニットテストを作成

**テストケース:**
- CM-001〜CM-009（テスト仕様書参照）

**受け入れ条件:**
- [x] Commitの全フィールドをパースできる
- [x] 親コミットを正しく取得できる
- [x] Signatureのタイムゾーンを正しくパースできる
- [x] テスト CM-001〜CM-009 がパス

**依存:** #007, #008

---

### Issue #012: Object enum実装 ✅

**説明:**
各オブジェクト型を統合するObject enumを実装する。

**タスク:**
- [x] `Object` enumを `src/objects/mod.rs` に定義
- [x] `kind()` メソッドを実装
- [x] `as_blob()`, `as_tree()`, `as_commit()` を実装
- [x] `into_blob()`, `into_tree()`, `into_commit()` を実装

**受け入れ条件:**
- [x] 任意のオブジェクトをObject enumで扱える
- [x] 型変換メソッドが正しく動作する

**依存:** #009, #010, #011

---

## Phase 1: Refs Layer

### Issue #013: 参照解決実装 ✅

**説明:**
HEADおよびブランチ参照の解決機能を実装する。

**タスク:**
- [x] `src/refs/mod.rs` を作成
- [x] `src/refs/resolver.rs` を作成
- [x] `RefStore` 構造体を定義
- [x] `read_ref_file()` を実装
- [x] `resolve_recursive()` を実装
- [x] `head()` を実装
- [x] `branches()` を実装
- [x] ユニットテストを作成

**テストケース:**
- R-001〜R-005（テスト仕様書参照）

**受け入れ条件:**
- [x] HEADを正しく解決できる
- [x] symbolic-refを再帰的に解決できる
- [x] ブランチ一覧を取得できる
- [x] テスト R-001〜R-005 がパス

**依存:** #006, #007

---

### Issue #014: Head / Branch 型実装 ✅

**説明:**
HEADとブランチを表す型を実装する。

**タスク:**
- [x] `src/refs/head.rs` を作成
- [x] `src/refs/branch.rs` を作成
- [x] `Head` enumを定義（Branch / Detached）
- [x] `Branch` 構造体を定義
- [x] 各メソッドを実装

**受け入れ条件:**
- [x] detached HEAD状態を正しく表現できる
- [x] ブランチ情報を取得できる

**依存:** #007, #013

---

## Phase 1: Index Layer

### Issue #015: Indexパース実装 ✅

**説明:**
.git/indexファイルのパース機能を実装する。

**タスク:**
- [x] `src/index/mod.rs` を作成
- [x] `src/index/reader.rs` を作成
- [x] `Index`, `IndexEntry` 構造体を定義
- [x] `parse()` を実装
- [x] `parse_header()` を実装
- [x] `parse_entry()` を実装
- [x] 各アクセサメソッドを実装
- [x] ユニットテストを作成

**テストケース:**
- I-001〜I-005（テスト仕様書参照）

**受け入れ条件:**
- [x] v2形式のIndexをパースできる
- [x] 全エントリを正しく取得できる
- [x] 不正な形式でエラーを返す
- [x] テスト I-001〜I-005 がパス

**依存:** #006, #007

---

## Phase 1: Repository Layer

### Issue #016: Repository基本実装 ✅

**説明:**
Repositoryの基本機能（open/discover）を実装する。

**タスク:**
- [x] `src/repository.rs` を作成
- [x] `Repository` 構造体を定義
- [x] `validate_git_dir()` を実装
- [x] `open()` を実装
- [x] `discover()` を実装
- [x] `path()`, `git_dir()` を実装
- [x] 統合テストを作成

**テストケース:**
- RP-001〜RP-007（テスト仕様書参照）

**受け入れ条件:**
- [x] 有効なリポジトリを開ける
- [x] 親ディレクトリを遡って.gitを発見できる
- [x] 無効なパスでエラーを返す
- [x] テスト RP-001〜RP-007 がパス

**依存:** #008, #013, #015

---

### Issue #017: Repository オブジェクト取得メソッド実装 ✅

**説明:**
commit/tree/blob/objectの取得メソッドを実装する。

**タスク:**
- [x] `resolve_short_oid()` を実装
- [x] `commit()` を実装
- [x] `tree()` を実装
- [x] `blob()` を実装
- [x] `object()` を実装
- [x] 統合テストを作成

**テストケース:**
- RP-010〜RP-013, RP-030〜RP-034（テスト仕様書参照）

**受け入れ条件:**
- [x] 完全SHA/短縮SHAでオブジェクトを取得できる
- [x] 型不一致でエラーを返す
- [x] テストがパス

**依存:** #012, #016

---

### Issue #018: LogIterator実装 ✅

**説明:**
コミット履歴を遅延取得するイテレータを実装する。

**タスク:**
- [x] `src/log.rs` を作成
- [x] `LogIterator` 構造体を定義
- [x] `PendingCommit` を定義（優先度キュー用）
- [x] `Iterator` トレイトを実装
- [x] `Repository::log()`, `log_from()` を実装
- [x] 統合テストを作成

**テストケース:**
- RP-014〜RP-016（テスト仕様書参照）

**受け入れ条件:**
- [x] HEADからコミット履歴を取得できる
- [x] 時刻降順（新しい順）で取得される
- [x] マージコミットを正しく辿る
- [x] テスト RP-014〜RP-016 がパス

**依存:** #014, #017

---

### Issue #019: Status実装 ✅

**説明:**
ワーキングツリーの状態取得機能を実装する。

**タスク:**
- [x] `src/status.rs` を作成
- [x] `StatusEntry`, `FileStatus` を定義
- [x] `flatten_tree()` を実装
- [x] `file_modified()` を実装
- [x] `compute_status()` を実装
- [x] `Repository::status()` を実装
- [x] 統合テストを作成

**テストケース:**
- RP-020〜RP-024（テスト仕様書参照）

**受け入れ条件:**
- [x] Untracked/Modified/Deleted/Stagedを正しく検出
- [x] HEAD/Index/Working treeの三方比較が機能する
- [x] テスト RP-020〜RP-024 がパス

**依存:** #015, #017

---

### Issue #020: 公開API整理とlib.rs実装 ✅

**説明:**
公開APIを整理し、lib.rsでre-exportする。

**タスク:**
- [x] `src/lib.rs` を整理
- [x] 公開する型をre-export
- [x] ドキュメントコメントを追加
- [x] クレートレベルのドキュメントを追加

**公開API:**
```rust
// src/lib.rs
pub use error::{Error, Result};
pub use repository::Repository;
pub use objects::{Object, Oid, Blob, Tree, TreeEntry, Commit, Signature, FileMode};
pub use refs::{Head, Branch};
pub use status::{StatusEntry, FileStatus};
pub use index::{Index, IndexEntry};
```

**受け入れ条件:**
- [x] `use zerogit::*;` で必要な型がインポートできる
- [x] `cargo doc` でドキュメントが生成される
- [x] READMEのコード例がコンパイルできる

**依存:** #016, #017, #018, #019

---

### Issue #021: MVP統合テストとリリース準備 ✅

**説明:**
Phase 1（MVP）の統合テストを実施し、リリース準備を行う。

**タスク:**
- [x] 統合テストを拡充
- [x] パフォーマンステストを実施
- [x] カバレッジを計測（80%目標）
- [x] `cargo clippy` の警告を解消
- [x] `cargo fmt` でフォーマット
- [x] CHANGELOG.md を作成
- [x] バージョンを 0.1.0 に設定

**受け入れ条件:**
- [x] 全テストがパス
- [x] カバレッジ80%以上
- [x] Clippy警告なし
- [x] READMEの全コード例が動作する

**依存:** #020

---

## Phase 2: 書き込み操作

### Issue #022: Index書き込み実装 ✅

**説明:**
Indexファイルの書き込み機能を実装する。

**タスク:**
- [x] `src/index/writer.rs` を作成
- [x] `Index::write()` を実装
- [x] `write_entry()` を実装
- [x] チェックサム計算を実装
- [x] ユニットテストを作成

**受け入れ条件:**
- [x] Indexをファイルに書き込める
- [x] gitコマンドで読み取れる形式で出力される

**依存:** #015, #004

---

### Issue #023: オブジェクト書き込み実装 ✅

**説明:**
Looseオブジェクトの書き込み機能を実装する。

**タスク:**
- [x] `compress()` 関数を実装（infra/compression.rs）
- [x] `LooseObjectStore::write()` を実装
- [x] ユニットテストを作成

**受け入れ条件:**
- [x] オブジェクトをファイルに書き込める
- [x] gitコマンドで読み取れる

**依存:** #008, #005

---

### Issue #024: Repository::add()実装 ✅

**説明:**
ファイルをステージングエリアに追加する機能を実装する。

**タスク:**
- [x] `Repository::add()` を実装
- [x] `Repository::add_all()` を実装
- [x] `Repository::reset()` を実装
- [x] 統合テストを作成

**テストケース:**
- W-001〜W-003（テスト仕様書参照）

**受け入れ条件:**
- [x] ファイルをステージできる
- [x] ステージを解除できる
- [x] `git status` で正しく反映される

**依存:** #022, #023

---

### Issue #025: Repository::create_commit()実装 ✅

**説明:**
コミット作成機能を実装する。

**タスク:**
- [x] `build_tree_from_index()` を実装
- [x] `format_commit()` を実装
- [x] `update_head()` を実装
- [x] `Repository::create_commit()` を実装
- [x] 統合テストを作成

**テストケース:**
- W-004〜W-005（テスト仕様書参照）

**受け入れ条件:**
- [x] コミットを作成できる
- [x] HEADが更新される
- [x] `git log` で表示される

**依存:** #023, #024

---

### Issue #026: ブランチ操作実装 ✅

**説明:**
ブランチの作成・削除・切り替え機能を実装する。

**タスク:**
- [x] `Repository::create_branch()` を実装
- [x] `Repository::delete_branch()` を実装
- [x] `Repository::checkout()` を実装
- [x] 統合テストを作成

**テストケース:**
- W-006〜W-010（テスト仕様書参照）

**受け入れ条件:**
- [x] ブランチを作成・削除できる
- [x] ブランチを切り替えられる
- [x] 現在のブランチは削除できない

**依存:** #013, #025

---

## Phase 2.5: 参照拡張・ログフィルタリング・差分機能

### Issue #027: リモートブランチ・タグ一覧対応 ✅

**説明:**
refs/remotes/* と refs/tags/* の読み取りに対応し、branches機能を完全にする。

**タスク:**
- [x] `src/refs/remote_branch.rs` を作成
- [x] `RemoteBranch` 型を定義（remote名 + branch名）
- [x] `Repository::remote_branches()` を実装
- [x] `src/refs/tag.rs` を作成
- [x] `Tag` 型を定義（軽量タグ / 注釈付きタグ対応）
- [x] `Repository::tags()` を実装
- [x] `RefStore` にremotes/tags走査を追加
- [x] 注釈付きタグ（tag object）のパースを実装
- [x] 統合テストを作成

**受け入れ条件:**
- [x] `git branch -r` 相当の出力が得られる
- [x] `git tag` 相当の出力が得られる
- [x] 注釈付きタグのメッセージを取得できる

**依存:** #013, #014

---

### Issue #028: ログフィルタリング・オプション対応 ✅

**説明:**
log機能を拡張し、パス指定やコミット数制限などのフィルタリングに対応する。

**タスク:**
- [x] `src/log.rs` に `LogOptions` ビルダーを追加
- [x] `Repository::log_with_options()` を実装
- [x] パスフィルタリングを実装
- [x] 件数制限を実装
- [x] 日付範囲フィルタを実装（since/until）
- [x] `--first-parent` 相当を実装
- [x] 作者フィルタを実装
- [x] 統合テストを作成

**受け入れ条件:**
- [x] 特定ファイルの変更履歴を取得できる
- [x] 件数制限が機能する
- [x] 日付範囲でフィルタできる
- [x] first-parentモードが機能する

**依存:** #018

---

### Issue #029: Tree Diff実装 ✅

**説明:**
2つのTree間の差分を計算する機能を実装。diff機能全体の基盤。

**タスク:**
- [x] `src/diff/mod.rs` を作成
- [x] `TreeDiff` 構造体を定義
- [x] `DiffDelta` 構造体を定義
- [x] `DiffStatus` enumを定義
- [x] Treeフラット化ユーティリティを実装
- [x] `diff_trees()` コア関数を実装
- [x] `Repository::diff_trees()` を実装
- [x] リネーム検出を実装
- [x] 統合テストを作成

**受け入れ条件:**
- [x] 追加・削除・変更ファイルを正しく検出できる
- [x] `git diff --name-status` 相当の出力が得られる
- [x] 空Tree（初期コミット）との比較ができる

**依存:** #010, #017

---

### Issue #030: コミット変更ファイル一覧 ✅

**説明:**
Commitから直接変更ファイル一覧を取得できるようにする。

**タスク:**
- [x] `Repository::commit_diff()` を実装
- [x] 親コミットとのTree差分を計算
- [x] 初期コミット（親なし）の処理
- [x] マージコミット（複数親）の処理
- [x] 統合テストを作成

**受け入れ条件:**
- [x] 通常のコミットで変更ファイル一覧を取得できる
- [x] 初期コミットで全ファイルがAddedとして表示される
- [x] `git log --name-status` 相当の出力が得られる

**依存:** #029

---

### Issue #031: Working Tree / Index Diff ✅

**説明:**
ワーキングツリーとIndex、IndexとHEADの差分を取得する機能。

**タスク:**
- [x] `Repository::diff_index_to_workdir()` を実装
- [x] `Repository::diff_head_to_index()` を実装
- [x] `Repository::diff_head_to_workdir()` を実装
- [x] ワーキングツリーファイルのハッシュ計算
- [x] 仮想Tree（Index/Workdirから）の構築
- [x] `status()` との整合性を確保
- [x] 統合テストを作成

**受け入れ条件:**
- [x] `git diff` 相当（未ステージ変更）が取得できる
- [x] `git diff --staged` 相当（ステージ済み変更）が取得できる
- [x] `git diff HEAD` 相当（全変更）が取得できる

**依存:** #015, #029

---

## マイルストーン

### Milestone 1: MVP（Phase 1完了）✅
- Issue #001〜#021
- 読み取り専用機能の完成
- v0.1.0 リリース

### Milestone 2: 書き込み対応（Phase 2完了）✅
- Issue #022〜#026
- add/commit/branch機能の完成
- v0.2.0 リリース

### Milestone 3: 参照拡張・差分機能（Phase 2.5完了）✅
- Issue #027〜#031
- リモートブランチ・タグ対応
- ログフィルタリング
- Tree diff・Working Tree diff
- v0.3.0 リリース

---

## 優先度・難易度マトリクス

| Issue | 優先度 | 難易度 | 推定時間 |
|-------|--------|--------|----------|
| #001 | 最高 | 低 | 1h |
| #002 | 最高 | 低 | 2h |
| #003 | 最高 | 低 | 1h |
| #004 | 高 | 中 | 4h |
| #005 | 高 | 低 | 1h |
| #006 | 高 | 低 | 2h |
| #007 | 高 | 低 | 2h |
| #008 | 高 | 中 | 4h |
| #009 | 高 | 低 | 2h |
| #010 | 高 | 中 | 3h |
| #011 | 高 | 中 | 3h |
| #012 | 中 | 低 | 1h |
| #013 | 高 | 中 | 3h |
| #014 | 中 | 低 | 1h |
| #015 | 高 | 高 | 5h |
| #016 | 高 | 低 | 2h |
| #017 | 高 | 低 | 2h |
| #018 | 中 | 中 | 3h |
| #019 | 高 | 高 | 5h |
| #020 | 中 | 低 | 2h |
| #021 | 中 | 中 | 4h |
| #022 | 高 | 中 | 3h |
| #023 | 高 | 中 | 2h |
| #024 | 高 | 中 | 3h |
| #025 | 高 | 高 | 4h |
| #026 | 高 | 中 | 3h |
| #027 | 高 | 低 | 3h |
| #028 | 中 | 中 | 4h |
| #029 | 高 | 中 | 5h |
| #030 | 高 | 低 | 2h |
| #031 | 中 | 中 | 3h |

**Phase 1 合計推定時間:** 約53時間
**Phase 2 合計推定時間:** 約15時間
**Phase 2.5 合計推定時間:** 約17時間

---

## 依存関係グラフ

```
#001 プロジェクト初期化
  │
  ├─> #002 エラー型
  │     │
  │     ├─> #004 SHA-1
  │     │     │
  │     │     └─> #007 Oid
  │     │           │
  │     │           ├─> #008 ObjectStore
  │     │           │     │
  │     │           │     ├─> #009 Blob
  │     │           │     ├─> #010 Tree
  │     │           │     └─> #011 Commit
  │     │           │           │
  │     │           │           └─> #012 Object enum
  │     │           │
  │     │           └─> #013 Refs
  │     │                 │
  │     │                 └─> #014 Head/Branch
  │     │
  │     ├─> #005 Compression
  │     │
  │     └─> #006 FileSystem
  │           │
  │           └─> #015 Index
  │
  └─> #003 Fixtures

#008, #013, #015 ──> #016 Repository基本
                          │
                          ├─> #017 オブジェクト取得
                          │     │
                          │     ├─> #018 LogIterator ──> #028 LogOptions
                          │     │
                          │     ├─> #019 Status
                          │     │
                          │     └─> #029 Tree Diff ──> #030 Commit Diff
                          │                       └─> #031 Workdir Diff
                          │
                          ├─> #020 公開API
                          │     │
                          │     └─> #021 MVP完了
                          │
                          ├─> #022 Index書き込み ──> #024 add
                          │                           │
                          │                           └─> #025 commit ──> #026 branch
                          │
                          └─> #027 RemoteBranch/Tag
```
