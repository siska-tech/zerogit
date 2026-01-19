# Changelog

このプロジェクトは [Keep a Changelog](https://keepachangelog.com/ja/1.0.0/) に準拠し、
[Semantic Versioning](https://semver.org/lang/ja/) を採用しています。

## [0.3.6] - 2026-01-20

### Fixed

#### ログのパスフィルタリング改善
- `log_with_options()`のパスフィルタリングがサブディレクトリ内のファイルを正しく検出するように修正
- ディレクトリプレフィックス指定（`src/`や`src`）で配下の全ファイルの変更を検出可能に
- ネストされたパス（`src/utils/helpers/mod.rs`など）のフィルタリングに対応

---

## [0.3.5] - 2026-01-20

### Added

#### リポジトリ初期化
- `Repository::init()`: 新規Gitリポジトリを初期化
- 必要なディレクトリ構造（`.git/objects`, `.git/refs/heads`, `.git/refs/tags`）を自動作成
- デフォルトブランチは `main`

#### Commit OID取得
- `Commit::oid()`: コミット自身のOIDを取得するメソッドを追加
- `Oid::short()`: 短縮形式（7文字）のOIDを取得

#### ローカルブランチ一覧
- `Repository::branches()`: ローカルブランチ一覧を`Vec<Branch>`として取得
- `remote_branches()`と対称的なAPIを提供

### Fixed
- `Repository::log()` で各コミットのOIDが取得可能に

---

## [0.3.0] - 2026-01-20

### Added

Phase 2.5: 参照拡張・ログフィルタリング・差分機能の完全実装。

#### リモートブランチ・タグ対応
- `Repository::remote_branches()`: リモートブランチ（refs/remotes/*）の一覧取得
- `Repository::tags()`: タグ（refs/tags/*）の一覧取得
- `RemoteBranch` 型: リモート名とブランチ名を分離して取得可能
- `Tag` 型: 軽量タグ・注釈付きタグ両対応、メッセージ・tagger情報取得可能
- 注釈付きタグオブジェクト（tag object）のパース対応

#### ログフィルタリング
- `Repository::log_with_options()`: フィルタリング付きログ取得
- `LogOptions` ビルダー: 柔軟なオプション指定
  - `path()` / `paths()`: 特定ファイル・ディレクトリの変更履歴
  - `max_count()`: 最大取得件数
  - `since()` / `until()`: 日付範囲フィルタ
  - `first_parent()`: マージの片側のみを辿る
  - `author()`: 作者名でフィルタ
  - `from()`: 開始コミット指定

#### Tree Diff
- `Repository::diff_trees()`: 2つのTree間の差分計算
- `TreeDiff` 型: 差分結果のコンテナ
- `DiffDelta` 型: 各変更エントリ（パス、ステータス、OID）
- `DiffStatus` enum: Added, Deleted, Modified, Renamed, Copied
- `DiffStats` 型: 変更ファイル数の統計
- 完全一致リネーム検出対応

#### コミット変更一覧
- `Repository::commit_diff()`: コミットの変更ファイル一覧取得
- 初期コミット（親なし）対応
- マージコミット対応（最初の親との差分）

#### ワーキングツリー・Index差分
- `Repository::diff_index_to_workdir()`: git diff 相当
- `Repository::diff_head_to_index()`: git diff --staged 相当
- `Repository::diff_head_to_workdir()`: git diff HEAD 相当

### Changed
- `LogIterator` 内部構造をフィルタリング対応に拡張

---

## [0.2.0] - 2026-01-18

### Added

Phase 2: 書き込み操作の完全実装。

#### ステージング操作
- `Repository::add()`: ファイルをステージングエリアに追加
- `Repository::add_all()`: 全変更（新規、変更、削除）をステージ
- `Repository::reset()`: ステージを解除（HEADの状態に戻す）

#### コミット作成
- `Repository::create_commit()`: インデックスからコミットを作成
- ツリーオブジェクトの自動構築（サブディレクトリ対応）
- 親コミットの自動検出とHEAD更新

#### ブランチ操作
- `Repository::create_branch()`: 新しいブランチを作成
- `Repository::delete_branch()`: ブランチを削除（現在のブランチは削除不可）
- `Repository::checkout()`: ブランチまたはコミットに切り替え
- ネストされたブランチ名のサポート（例: `feature/foo`）
- detached HEAD状態への切り替え対応

#### インデックス書き込み
- `Index::write()`: インデックスをファイルに書き込み
- `Index::add()`: エントリを追加/更新
- `Index::remove()`: エントリを削除
- `Index::empty()`: 空のインデックスを作成
- チェックサム計算とv2形式での出力

#### オブジェクト書き込み
- `LooseObjectStore::write()`: looseオブジェクトの書き込み
- `compress()`: zlibフォーマットでの圧縮
- 冪等性の保証（既存オブジェクトは再書き込みしない）

### Changed
- `Index` 構造体に可変操作メソッドを追加

---

## [0.1.0] - 2026-01-17

### Added

Phase 1: Repository Layer（読み取り操作）の完全実装。

#### オブジェクト操作
- `Oid`: SHA-1オブジェクトID型（16進文字列変換、短縮形式対応）
- `Blob`: blobオブジェクト（ファイル内容）の読み取り
- `Tree`: treeオブジェクト（ディレクトリ構造）の読み取り
- `Commit`: commitオブジェクト（コミット情報）の読み取り
- `LooseObjectStore`: loose objectの読み取りと前方一致検索

#### 参照解決
- `RefStore`: 参照ファイルの読み取りとシンボリック参照の解決
- `Head`: HEAD参照（ブランチまたはdetached HEAD）
- `Branch`: ブランチ情報と一覧取得
- タグ一覧の取得

#### リポジトリ操作
- `Repository::open()`: 指定パスでリポジトリを開く
- `Repository::discover()`: 親ディレクトリを探索してリポジトリを発見
- `Repository::commit()`: 短縮SHA-1でコミットを取得
- `Repository::tree()`: ツリーオブジェクトを取得
- `Repository::blob()`: blobオブジェクトを取得
- `Repository::head()`: HEAD参照を取得
- `Repository::branches()`: ブランチ一覧を取得
- `Repository::log()`: コミット履歴をイテレート
- `Repository::status()`: ワーキングツリーの状態を取得

#### インデックス
- Git index（.git/index）のパース（v2/v3/v4対応）
- インデックスエントリの読み取り

#### ステータス
- Untracked files（未追跡ファイル）の検出
- Modified files（変更ファイル）の検出
- Deleted files（削除ファイル）の検出
- Staged changes（ステージされた変更）の検出

#### インフラストラクチャ
- Pure Rust SHA-1実装
- zlib解凍（miniz_oxide使用）
- ファイルシステムユーティリティ

### Dependencies
- `miniz_oxide` 0.8 - zlib解凍

### Notes
- 最小Rustバージョン: 1.70.0
- 対応プラットフォーム: Linux, macOS, Windows
- テストカバレッジ: 94%以上

[0.3.6]: https://github.com/siska-tech/zerogit/releases/tag/v0.3.6
[0.3.5]: https://github.com/siska-tech/zerogit/releases/tag/v0.3.5
[0.3.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.3.0
[0.2.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.2.0
[0.1.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.1.0
