# Changelog

このプロジェクトは [Keep a Changelog](https://keepachangelog.com/ja/1.0.0/) に準拠し、
[Semantic Versioning](https://semver.org/lang/ja/) を採用しています。

## [Unreleased]

### Added
- `Error::UnmergedPaths`: indexにコンフリクト（stage 1〜3）が残っている場合のエラー
- `Index::get_stage()`、`Index::has_conflicts()`、`Index::conflicted_paths()`
- `.gitignore`・`.git/info/exclude`・`core.excludesFile`の解釈（否定、ディレクトリ指定、固定、`*`・`?`・`[...]`・`**`、エスケープ、`core.ignoreCase`）。status・`add_all`・作業ツリーとの差分に適用する (#20)
- `Repository::is_ignored()`、`Repository::ignored_files()`、`Repository::add_force()`
- `Error::IgnoredPath`: 無視された未追跡ファイルを`add`した場合のエラー
- 改行コードの変換: `core.autocrlf`（true/input/false）・`core.eol`・`.gitattributes`（`text`、`text=auto`、`-text`、`binary`、`eol=lf|crlf`、旧形式の`crlf`）・`core.attributesFile`・`.git/info/attributes`に従い、`add`/`add_all`・status・作業ツリーとの差分ではCRLF→LF、`checkout`ではLF→CRLFに変換する。`text=auto`の判定とindexにCRLFがあるファイルの扱いはGitと同じ (#21)
- `core.safecrlf=true`で往復できない変換（改行の混在など）を`add`すると`Error::IrreversibleLineEndings`を返す。既定（warn）では変換して続行する
- `filter`（driverが設定されている場合）・`ident`・`working-tree-encoding`属性は未対応として扱い、`add`/`checkout`は`Error::UnsupportedAttribute`を返す。statusと作業ツリーとの差分は、サイズと更新時刻がindexと一致すれば変更なしとし、一致しなければ変更ありとする。driverが設定されていない`filter`はGitと同じく無視する
- `Repository::detailed_status()`: `git status --porcelain=v2`と同じく、index側（HEAD→index）と作業ツリー側（index→作業ツリー）の状態をパスごとに別々に返す。コンフリクトの種類（`UU`・`AA`・`DU`など）、intent-to-add（`.A`）、型の変更（`T`）、ステージ済みの追加後の削除（`AD`）を区別する (#24)
- `DetailedStatus`、`DetailedStatusEntry`、`ChangeState`、`ConflictKind`
- 設定の読み込みで`GIT_CONFIG_NOSYSTEM`・`GIT_CONFIG_SYSTEM`・`GIT_CONFIG_GLOBAL`に従う
- symlinkをGitと同じくmode `120000`・リンク先パスを内容とするBlobとして扱う。`add`/`add_all`・status・作業ツリーとの差分・`checkout`に対応。リンク切れも`add`できる。`core.symlinks=false`や作成できない環境では、リンク先パスを内容とする通常ファイルとして書き出す (#22)

### Changed
- crates.ioのパッケージから`issues/`・`docs/`・`tests/`を除外した
- **破壊的変更**: `Error`に`UnmergedPaths`・`IgnoredPath`・`UnsupportedAttribute`・`IrreversibleLineEndings`を追加した
- グローバル設定は`$XDG_CONFIG_HOME/git/config`の後に`~/.gitconfig`を読む（Gitと同じく`~/.gitconfig`が優先）
- 作業ツリーの走査で、名前が`.`で始まるファイル・ディレクトリ（`.github/`、`.env.example`など）を除外しない。除外するのは`.git`だけ (#20)
- `Repository::add`は、無視された未追跡ファイルを`IgnoredPath`で拒否する（`git add`と同じ）
- status・作業ツリーとの差分は、内容に加えてmode（実行ビット、symlink）の違いも変更として報告する。`core.fileMode=false`（およびUnix以外）ではindexのmodeを使う
- `checkout`は実行ビットを設定し、ファイルとsymlinkの置き換えに対応する
- indexに書くサイズを作業ツリー上のファイルサイズにし、時刻のナノ秒も記録する
- `Index::add`はstage 0の追加で同じパスの全stageを置き換え、`Index::remove`は全stageを削除する。エントリはGitと同じ順（パスのバイト列、次にstage）に保つ
- `Repository::add`は、作業ツリーから削除された追跡中のファイルの削除をステージする（`git add`と同じ）
- `Repository::add_all`は、HEADにないファイルを含め、作業ツリーにないindexのエントリを削除する（`git add -A`と同じ）

### Fixed
- 作業ツリーの走査がディレクトリへのsymlinkを辿らない（リポジトリ外の走査や無限ループを防ぐ）(#22)
- コンフリクト中のindexから`create_commit`すると、同名エントリが重複した不正なtreeを作っていた。`UnmergedPaths`を返し、何も書き込まない (#23)
- コンフリクト中の`checkout`を`UnmergedPaths`で拒否する (#23)
- `status()`がコンフリクト中のパスをstage 3の内容で比較していた。`Modified`として1件報告する (#23)
- treeのエントリを名前順に並べていたため、`foo`ディレクトリと`foo.txt`などが並ぶとGitの順序（ディレクトリは`foo/`として比較）と異なり、`git fsck`が不正と判定するtreeを作っていた

---

## [0.4.0] - 2026-10-04

### Added

#### Packfile・packed-refs読み取り
- pack v2/v3とidx v2（64bit offsetを含む）の読み取り。OFS_DELTA/REF_DELTAと多段deltaの復元に対応
- looseと複数packを横断して読み取る（read・存在確認・短縮OID検索）。外部の`git repack`/`git gc`にも追従
- `packed-refs`によるHEAD・ブランチ・リモートブランチ・タグの解決と一覧
- `objects::pack::{PackIndex, PackFile, PackLimits}`

#### 行単位差分
- `Repository::diff_blobs()`、`BlobDiff::compute()`: 旧新行番号付きの行差分（Myers法）
- `BlobDiff`、`BlobDiffContent`、`DiffHunk`、`DiffLine`、`DiffOptions`、`LineKind`、`LineEnding`、`NonTextReason`、`SkipReason`
- 利用例 `examples/document_diff.rs`、測定 `examples/measure_document_diff.rs`

#### 類似度リネーム検出（任意）
- `Repository::diff_trees_with_options()`、`Repository::commit_diff_with_options()`、`RenameOptions`、`RenameDetection`、`RenameLimit`
- `DiffDelta::similarity()`（完全一致は100）、`TreeDiff::rename_limits()`

#### エラー
- `InvalidPackedRefs`、`PackedRefDeletionUnsupported`、`InvalidPackIndex`、`UnsupportedPackIndexVersion`、`InvalidPack`、`UnsupportedPackVersion`、`UnsupportedRepositoryFormat`、`UnsupportedIndex`、`PackLimitExceeded`
- `IndexEntry::ctime_nsec()`、`mtime_nsec()`、`skip_worktree()`、`intent_to_add()`

### Changed
- **破壊的変更**: `Error`にvariantを9つ追加した（上記「エラー」）。`Error`を網羅的に`match`しているコードは、新しいvariantへの対応が必要
- `Repository::open`/`discover`/`init`は`.git/config`を読み、SHA-256（`extensions.objectFormat`）・reftable・未知の`repositoryformatversion`を`UnsupportedRepositoryFormat`として拒否する。configが壊れている場合もopen時にエラーになる
- packed参照を含むブランチの削除は`PackedRefDeletionUnsupported`を返す

### Fixed
- 完全一致リネームで、移動と同時に実行権限が変わった場合も旧新modeを正しく保持する。同じ内容のファイルが複数ある場合の対応付けを決定的・一対一にした
- 製品コードでI/Oエラーや破損を「オブジェクトなし」として扱っていた箇所を、明示的なエラーにした
- index v4（パス圧縮）を正しく読み書きする。以前はv4を誤解析し、書き戻すとGitが読めないindexになっていた（#16）。読んだバージョンで書き戻すため、ヘッダとエントリの形式は常に一致する
- index読み取りでチェックサム・パディング・名前の終端・拡張フラグを検証する。nanosecond時刻とskip-worktree・intent-to-addのフラグを書き戻しで保持する
- split index・sparse indexは`Error::UnsupportedIndex`として明示的に拒否する（誤読・破損書き込みを防止）。skip-worktreeを含むindexの全体reset・checkoutも同様に拒否する
- intent-to-add（`git add -N`）のエントリをコミットに含めない。skip-worktreeのエントリをstatus・作業ツリー差分で削除扱いにしない
- reset・checkoutでindexを作り直す際に、実行権限・symlinkのmodeを失っていた問題を修正

---

## [0.3.7] - 2026-01-20

### Added

#### Git Config読み取り
- `Repository::config()`: リポジトリの設定を取得
- `Config::get()`: 設定値をキーで取得（`user.name`, `user.email`など）
- ローカル設定（`.git/config`）の読み取りに対応

---

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

[0.4.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.4.0
[0.3.7]: https://github.com/siska-tech/zerogit/releases/tag/v0.3.7
[0.3.6]: https://github.com/siska-tech/zerogit/releases/tag/v0.3.6
[0.3.5]: https://github.com/siska-tech/zerogit/releases/tag/v0.3.5
[0.3.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.3.0
[0.2.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.2.0
[0.1.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.1.0
