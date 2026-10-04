# zerogit インターフェース設計書

## 1. 公開API一覧

### 1.1 モジュール構成

```rust
pub mod zerogit {
    // コアAPI
    pub struct Repository;
    pub struct Oid;
    pub struct Config;

    // オブジェクト
    pub enum Object;
    pub struct Blob;
    pub struct Tree;
    pub struct TreeEntry;
    pub struct Commit;
    pub struct Signature;

    // 参照
    pub enum Head;
    pub struct Branch;
    pub struct RemoteBranch;
    pub struct Tag;

    // ステータス
    pub struct StatusEntry;
    pub enum FileStatus;

    // インデックス
    pub struct Index;
    pub struct IndexEntry;

    // 差分
    pub struct TreeDiff;
    pub struct DiffDelta;
    pub enum DiffStatus;
    pub struct DiffStats;

    // 行差分（Phase 3）
    pub struct BlobDiff;
    pub enum BlobDiffContent;
    pub struct DiffHunk;
    pub struct DiffLine;
    pub struct DiffOptions;
    pub enum LineKind;
    pub enum LineEnding;
    pub enum NonTextReason;
    pub enum SkipReason;

    // Pack（Phase 3、zerogit::objects::pack）
    // pub struct PackIndex; pub struct PackIndexEntry;
    // pub struct PackFile; pub struct PackLimits; pub type BaseResolver;

    // ログ
    pub struct LogOptions;

    // エラー
    pub enum Error;
    pub type Result<T> = std::result::Result<T, Error>;

    // 定数
    pub enum FileMode;
}
```

### 1.2 公開要素サマリー

| カテゴリ     | 名前           | 種別   | Phase |
| ------------ | -------------- | ------ | ----- |
| コア         | `Repository`   | struct | 1     |
| コア         | `Oid`          | struct | 1     |
| コア         | `Config`       | struct | 2.5   |
| オブジェクト | `Object`       | enum   | 1     |
| オブジェクト | `Blob`         | struct | 1     |
| オブジェクト | `Tree`         | struct | 1     |
| オブジェクト | `TreeEntry`    | struct | 1     |
| オブジェクト | `Commit`       | struct | 1     |
| オブジェクト | `Signature`    | struct | 1     |
| 参照         | `Head`         | enum   | 1     |
| 参照         | `Branch`       | struct | 1     |
| 参照         | `RemoteBranch` | struct | 2.5   |
| 参照         | `Tag`          | struct | 2.5   |
| ステータス   | `StatusEntry`  | struct | 1     |
| ステータス   | `FileStatus`   | enum   | 1     |
| インデックス | `Index`        | struct | 1     |
| インデックス | `IndexEntry`   | struct | 1     |
| 差分         | `TreeDiff`     | struct | 2.5   |
| 差分         | `DiffDelta`    | struct | 2.5   |
| 差分         | `DiffStatus`   | enum   | 2.5   |
| 差分         | `DiffStats`    | struct | 2.5   |
| ログ         | `LogOptions`   | struct | 2.5   |
| 行差分       | `BlobDiff`     | struct | 3     |
| 行差分       | `BlobDiffContent` | enum | 3     |
| 行差分       | `DiffHunk`     | struct | 3     |
| 行差分       | `DiffLine`     | struct | 3     |
| 行差分       | `DiffOptions`  | struct | 3     |
| 行差分       | `LineKind` / `LineEnding` / `NonTextReason` / `SkipReason` | enum | 3 |
| Pack         | `objects::pack::{PackIndex, PackFile, PackLimits}` | struct | 3 |
| エラー       | `Error`        | enum   | 1     |
| 定数         | `FileMode`     | enum   | 1     |

---

## 2. 各APIの詳細定義

### 2.1 Repository

リポジトリ操作の中心となる構造体。

```rust
pub struct Repository { /* private fields */ }
```

#### コンストラクタ

##### `Repository::open`

```rust
pub fn open<P: AsRef<Path>>(path: P) -> Result<Repository>
```

| 項目   | 説明                                                              |
| ------ | ----------------------------------------------------------------- |
| 概要   | 指定パスの`.git`ディレクトリを持つリポジトリを開く                |
| 引数   | `path` - リポジトリのルートパス、または`.git`ディレクトリへのパス |
| 戻り値 | `Ok(Repository)` - 成功時                                         |
| エラー | `Error::NotARepository` - 有効なGitリポジトリではない             |
| エラー | `Error::Io` - ファイルアクセスエラー                              |

##### `Repository::discover`

```rust
pub fn discover<P: AsRef<Path>>(path: P) -> Result<Repository>
```

| 項目   | 説明                                                     |
| ------ | -------------------------------------------------------- |
| 概要   | 指定パスから親ディレクトリを遡り、`.git`を探索して開く   |
| 引数   | `path` - 検索開始パス                                    |
| 戻り値 | `Ok(Repository)` - 成功時                                |
| エラー | `Error::NotARepository` - ルートまで遡っても見つからない |
| エラー | `Error::Io` - ファイルアクセスエラー                     |

##### `Repository::init`（Phase 2.5）

```rust
pub fn init<P: AsRef<Path>>(path: P) -> Result<Repository>
```

| 項目   | 説明                                      |
| ------ | ----------------------------------------- |
| 概要   | 指定パスに新しいGitリポジトリを初期化     |
| 引数   | `path` - リポジトリを作成するパス         |
| 戻り値 | `Ok(Repository)` - 作成されたリポジトリ   |
| エラー | `Error::Io` - ディレクトリ作成エラー      |

#### メソッド（読み取り - Phase 1）

##### `Repository::head`

```rust
pub fn head(&self) -> Result<Head>
```

| 項目   | 説明                                                    |
| ------ | ------------------------------------------------------- |
| 概要   | 現在のHEADを取得                                        |
| 引数   | なし                                                    |
| 戻り値 | `Ok(Head)` - HEADの状態                                 |
| エラー | `Error::RefNotFound` - HEADが存在しない（空リポジトリ） |

##### `Repository::branches`

```rust
pub fn branches(&self) -> Result<Vec<Branch>>
```

| 項目   | 説明                                   |
| ------ | -------------------------------------- |
| 概要   | ローカルブランチ一覧を取得             |
| 引数   | なし                                   |
| 戻り値 | `Ok(Vec<Branch>)` - ブランチのリスト   |
| エラー | `Error::Io` - refs/heads読み取りエラー |

##### `Repository::commit`

```rust
pub fn commit(&self, id: &str) -> Result<Commit>
```

| 項目   | 説明                                                      |
| ------ | --------------------------------------------------------- |
| 概要   | 指定されたIDのコミットを取得                              |
| 引数   | `id` - SHA-1ハッシュ（完全形式または短縮形式、最低4文字） |
| 戻り値 | `Ok(Commit)` - コミット情報                               |
| エラー | `Error::InvalidOid` - 不正なハッシュ形式                  |
| エラー | `Error::ObjectNotFound` - オブジェクトが存在しない        |
| エラー | `Error::TypeMismatch` - オブジェクトがコミットではない    |

##### `Repository::log`

```rust
pub fn log(&self) -> Result<LogIterator<'_>>
```

| 項目   | 説明                                                 |
| ------ | ---------------------------------------------------- |
| 概要   | HEADからのコミット履歴イテレータを取得               |
| 引数   | なし                                                 |
| 戻り値 | `Ok(LogIterator)` - コミットを遅延取得するイテレータ |
| エラー | `Error::RefNotFound` - HEADが存在しない              |

##### `Repository::log_from`

```rust
pub fn log_from(&self, id: &str) -> Result<LogIterator<'_>>
```

| 項目   | 説明                                                 |
| ------ | ---------------------------------------------------- |
| 概要   | 指定コミットからの履歴イテレータを取得               |
| 引数   | `id` - 開始コミットのSHA-1                           |
| 戻り値 | `Ok(LogIterator)` - コミットを遅延取得するイテレータ |
| エラー | `Error::InvalidOid` - 不正なハッシュ形式             |
| エラー | `Error::ObjectNotFound` - 開始コミットが存在しない   |

##### `Repository::log_with_options`（Phase 2.5）

```rust
pub fn log_with_options(&self, options: LogOptions) -> Result<LogIterator<'_>>
```

| 項目   | 説明                                                 |
| ------ | ---------------------------------------------------- |
| 概要   | フィルタリングオプション付きでコミット履歴を取得     |
| 引数   | `options` - フィルタリングオプション                 |
| 戻り値 | `Ok(LogIterator)` - コミットを遅延取得するイテレータ |
| エラー | `Error::RefNotFound` - HEADが存在しない              |

##### `Repository::status`

```rust
pub fn status(&self) -> Result<Vec<StatusEntry>>
```

| 項目   | 説明                                               |
| ------ | -------------------------------------------------- |
| 概要   | ワーキングツリーの状態を取得                       |
| 引数   | なし                                               |
| 戻り値 | `Ok(Vec<StatusEntry>)` - 変更のあるファイル一覧    |
| エラー | `Error::Io` - ファイルシステムエラー               |
| エラー | `Error::InvalidIndex` - インデックス読み取りエラー |

作業ツリーの走査は`.git`だけを除外し、名前が`.`で始まるファイルも対象にする。未追跡ファイルには`.gitignore`（各ディレクトリ）・`.git/info/exclude`・`core.excludesFile`（既定は`$XDG_CONFIG_HOME/git/ignore`）をGitと同じ規則・優先順位で適用し、無視されたファイルは`Untracked`に含めない（`git ls-files --others --exclude-standard`と一致）。追跡中のファイルは無視指定に一致しても通常どおり比較する。

作業ツリーのファイルは内容とmodeの両方で比較する。symlinkは辿らず、リンク先パス（区切りは`/`）を内容とするmode `120000`として扱う。実行ビットは`core.fileMode`がtrue（既定）のUnixでのみ参照し、それ以外ではindexのmodeを使う。`checkout`は`core.symlinks`がtrue（既定）ならsymlinkを作成し、falseの場合や作成できない場合はリンク先パスを内容とする通常ファイルを書き出す（indexのmodeは`120000`のまま）。`core.ignoreCase`がtrueなら大文字・小文字を区別しない。

##### `Repository::is_ignored` / `Repository::ignored_files`

```rust
pub fn is_ignored<P: AsRef<Path>>(&self, path: P) -> Result<bool>
pub fn ignored_files(&self) -> Result<Vec<PathBuf>>
```

| 項目   | 説明 |
| ------ | ---- |
| 概要   | `is_ignored`: パスまたはその親ディレクトリが無視指定に一致するか（`git check-ignore --no-index`相当）。追跡中かどうかは考慮しない |
| 概要   | `ignored_files`: 無視された未追跡ファイルの一覧（`git ls-files --others --ignored --exclude-standard`相当） |

##### `Repository::object`

```rust
pub fn object(&self, id: &str) -> Result<Object>
```

| 項目   | 説明                                                |
| ------ | --------------------------------------------------- |
| 概要   | 任意のGitオブジェクトを取得                         |
| 引数   | `id` - SHA-1ハッシュ                                |
| 戻り値 | `Ok(Object)` - オブジェクト（Blob/Tree/Commit/Tag） |
| エラー | `Error::InvalidOid` - 不正なハッシュ形式            |
| エラー | `Error::ObjectNotFound` - オブジェクトが存在しない  |

##### `Repository::tree`

```rust
pub fn tree(&self, id: &str) -> Result<Tree>
```

| 項目   | 説明                                               |
| ------ | -------------------------------------------------- |
| 概要   | 指定されたIDのTreeを取得                           |
| 引数   | `id` - SHA-1ハッシュ                               |
| 戻り値 | `Ok(Tree)` - ツリー情報                            |
| エラー | `Error::ObjectNotFound` - オブジェクトが存在しない |
| エラー | `Error::TypeMismatch` - オブジェクトがTreeではない |

##### `Repository::blob`

```rust
pub fn blob(&self, id: &str) -> Result<Blob>
```

| 項目   | 説明                                               |
| ------ | -------------------------------------------------- |
| 概要   | 指定されたIDのBlobを取得                           |
| 引数   | `id` - SHA-1ハッシュ                               |
| 戻り値 | `Ok(Blob)` - ファイル内容                          |
| エラー | `Error::ObjectNotFound` - オブジェクトが存在しない |
| エラー | `Error::TypeMismatch` - オブジェクトがBlobではない |

##### `Repository::index`

```rust
pub fn index(&self) -> Result<Index>
```

| 項目   | 説明                                               |
| ------ | -------------------------------------------------- |
| 概要   | 現在のインデックス（ステージングエリア）を取得     |
| 引数   | なし                                               |
| 戻り値 | `Ok(Index)` - インデックス情報                     |
| エラー | `Error::InvalidIndex` - インデックス読み取りエラー |
| エラー | `Error::Io` - ファイルアクセスエラー               |

##### `Repository::path`

```rust
pub fn path(&self) -> &Path
```

| 項目   | 説明                         |
| ------ | ---------------------------- |
| 概要   | リポジトリのルートパスを取得 |
| 引数   | なし                         |
| 戻り値 | リポジトリルートへの参照     |

##### `Repository::git_dir`

```rust
pub fn git_dir(&self) -> &Path
```

| 項目   | 説明                           |
| ------ | ------------------------------ |
| 概要   | `.git`ディレクトリのパスを取得 |
| 引数   | なし                           |
| 戻り値 | `.git`ディレクトリへの参照     |

#### メソッド（書き込み - Phase 2）

##### `Repository::add`

```rust
pub fn add<P: AsRef<Path>>(&self, path: P) -> Result<()>
```

| 項目   | 説明                                                                |
| ------ | ------------------------------------------------------------------- |
| 概要   | ファイルをステージングエリアに追加                                  |
| 引数   | `path` - ステージするファイルパス（リポジトリルートからの相対パス） |
| 戻り値 | `Ok(())` - 成功時                                                   |
| エラー | `Error::PathNotFound` - ファイルが存在しない（追跡中のファイルが削除されている場合は、削除をステージする） |
| エラー | `Error::IgnoredPath` - 未追跡で無視指定に一致する（`git add`と同様）。`add_force`で追加できる |
| エラー | `Error::Io` - ファイル読み取りエラー                                |

`Repository::add_force`は無視指定を確認しない（`git add -f`相当）。

##### `Repository::add_all`

```rust
pub fn add_all(&self) -> Result<()>
```

| 項目   | 説明                                 |
| ------ | ------------------------------------ |
| 概要   | 変更のあるすべてのファイルをステージ |
| 引数   | なし                                 |
| 戻り値 | `Ok(())` - 成功時                    |
| エラー | `Error::Io` - ファイルシステムエラー |

`git add -A`相当。無視された未追跡ファイルは追加せず、作業ツリーにないindexのエントリは削除する。

##### `Repository::reset`

```rust
pub fn reset<P: AsRef<Path>>(&self, path: P) -> Result<()>
```

| 項目   | 説明                                                   |
| ------ | ------------------------------------------------------ |
| 概要   | ファイルをステージングエリアから除外                   |
| 引数   | `path` - アンステージするファイルパス                  |
| 戻り値 | `Ok(())` - 成功時                                      |
| エラー | `Error::PathNotFound` - パスがインデックスに存在しない |

##### `Repository::create_commit`

```rust
pub fn create_commit(
    &self,
    message: &str,
    author: Option<&Signature>,
    committer: Option<&Signature>,
) -> Result<Oid>
```

| 項目   | 説明                                                     |
| ------ | -------------------------------------------------------- |
| 概要   | 新しいコミットを作成                                     |
| 引数   | `message` - コミットメッセージ                           |
| 引数   | `author` - 作成者（Noneの場合はgit configから取得）      |
| 引数   | `committer` - コミッター（Noneの場合はauthorと同じ）     |
| 戻り値 | `Ok(Oid)` - 作成されたコミットのID                       |
| エラー | `Error::EmptyCommit` - ステージされた変更がない          |
| エラー | `Error::ConfigNotFound` - author未指定でgit config未設定 |
| エラー | `Error::UnmergedPaths` - indexにコンフリクト（stage 1〜3）が残っている。何も書き込まない |

##### `Repository::create_branch`

```rust
pub fn create_branch(&self, name: &str) -> Result<Branch>
```

| 項目   | 説明                                               |
| ------ | -------------------------------------------------- |
| 概要   | 現在のHEADから新しいブランチを作成                 |
| 引数   | `name` - ブランチ名                                |
| 戻り値 | `Ok(Branch)` - 作成されたブランチ                  |
| エラー | `Error::InvalidRefName` - 不正なブランチ名         |
| エラー | `Error::RefAlreadyExists` - 同名ブランチが既に存在 |

##### `Repository::delete_branch`

```rust
pub fn delete_branch(&self, name: &str) -> Result<()>
```

| 項目   | 説明                                                          |
| ------ | ------------------------------------------------------------- |
| 概要   | ブランチを削除                                                |
| 引数   | `name` - ブランチ名                                           |
| 戻り値 | `Ok(())` - 成功時                                             |
| エラー | `Error::RefNotFound` - ブランチが存在しない                   |
| エラー | `Error::CannotDeleteCurrentBranch` - 現在のブランチは削除不可 |

##### `Repository::checkout`

```rust
pub fn checkout(&self, name: &str) -> Result<()>
```

| 項目   | 説明                                               |
| ------ | -------------------------------------------------- |
| 概要   | ブランチを切り替え                                 |
| 引数   | `name` - ブランチ名                                |
| 戻り値 | `Ok(())` - 成功時                                  |
| エラー | `Error::RefNotFound` - ブランチが存在しない        |
| エラー | `Error::DirtyWorkingTree` - 未コミットの変更がある |

#### メソッド（Phase 2.5 追加）

##### `Repository::remote_branches`

```rust
pub fn remote_branches(&self) -> Result<Vec<RemoteBranch>>
```

| 項目   | 説明                                      |
| ------ | ----------------------------------------- |
| 概要   | リモートブランチ一覧を取得                |
| 引数   | なし                                      |
| 戻り値 | `Ok(Vec<RemoteBranch>)` - リモートブランチのリスト |
| エラー | `Error::Io` - refs/remotes読み取りエラー  |

##### `Repository::tags`

```rust
pub fn tags(&self) -> Result<Vec<Tag>>
```

| 項目   | 説明                                |
| ------ | ----------------------------------- |
| 概要   | タグ一覧を取得                      |
| 引数   | なし                                |
| 戻り値 | `Ok(Vec<Tag>)` - タグのリスト       |
| エラー | `Error::Io` - refs/tags読み取りエラー |

##### `Repository::diff_trees`

```rust
pub fn diff_trees(&self, old_tree: Option<&Tree>, new_tree: &Tree) -> Result<TreeDiff>
```

| 項目   | 説明                                           |
| ------ | ---------------------------------------------- |
| 概要   | 2つのTree間の差分を計算                        |
| 引数   | `old_tree` - 比較元Tree（Noneで空Tree扱い）    |
| 引数   | `new_tree` - 比較先Tree                        |
| 戻り値 | `Ok(TreeDiff)` - 差分情報                      |
| エラー | `Error::Io` - ファイルアクセスエラー           |

##### `Repository::commit_diff`

```rust
pub fn commit_diff(&self, commit: &Commit) -> Result<TreeDiff>
```

| 項目   | 説明                                           |
| ------ | ---------------------------------------------- |
| 概要   | コミットの変更ファイル一覧を取得               |
| 引数   | `commit` - 対象コミット                        |
| 戻り値 | `Ok(TreeDiff)` - 親コミットとの差分            |
| エラー | `Error::ObjectNotFound` - Treeが見つからない   |

##### `Repository::diff_index_to_workdir`

```rust
pub fn diff_index_to_workdir(&self) -> Result<TreeDiff>
```

| 項目   | 説明                                           |
| ------ | ---------------------------------------------- |
| 概要   | IndexとワーキングツリーのDiff（git diff相当）  |
| 引数   | なし                                           |
| 戻り値 | `Ok(TreeDiff)` - 未ステージの変更              |
| エラー | `Error::Io` - ファイルアクセスエラー           |

##### `Repository::diff_head_to_index`

```rust
pub fn diff_head_to_index(&self) -> Result<TreeDiff>
```

| 項目   | 説明                                              |
| ------ | ------------------------------------------------- |
| 概要   | HEADとIndexのDiff（git diff --staged相当）        |
| 引数   | なし                                              |
| 戻り値 | `Ok(TreeDiff)` - ステージ済みの変更               |
| エラー | `Error::RefNotFound` - HEADが存在しない           |

##### `Repository::diff_head_to_workdir`

```rust
pub fn diff_head_to_workdir(&self) -> Result<TreeDiff>
```

| 項目   | 説明                                              |
| ------ | ------------------------------------------------- |
| 概要   | HEADとワーキングツリーのDiff（git diff HEAD相当） |
| 引数   | なし                                              |
| 戻り値 | `Ok(TreeDiff)` - 全変更                           |
| エラー | `Error::RefNotFound` - HEADが存在しない           |

##### `Repository::config`

```rust
pub fn config(&self) -> Result<Config>
```

| 項目   | 説明                                  |
| ------ | ------------------------------------- |
| 概要   | リポジトリの設定を取得                |
| 引数   | なし                                  |
| 戻り値 | `Ok(Config)` - 設定情報               |
| エラー | `Error::Io` - configファイル読み取りエラー |

---

### 2.2 Oid

オブジェクトID（SHA-1ハッシュ）を表す構造体。

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Oid([u8; 20]);
```

#### コンストラクタ

##### `Oid::from_hex`

```rust
pub fn from_hex(s: &str) -> Result<Oid>
```

| 項目   | 説明                                           |
| ------ | ---------------------------------------------- |
| 概要   | 16進数文字列からOidを生成                      |
| 引数   | `s` - 40文字の16進数文字列                     |
| 戻り値 | `Ok(Oid)` - 成功時                             |
| エラー | `Error::InvalidOid` - 不正な形式（長さ、文字） |

##### `Oid::from_bytes`

```rust
pub fn from_bytes(bytes: &[u8]) -> Result<Oid>
```

| 項目   | 説明                                       |
| ------ | ------------------------------------------ |
| 概要   | 20バイト配列からOidを生成                  |
| 引数   | `bytes` - 20バイトのスライス               |
| 戻り値 | `Ok(Oid)` - 成功時                         |
| エラー | `Error::InvalidOid` - 長さが20バイトでない |

#### メソッド

##### `Oid::to_hex`

```rust
pub fn to_hex(&self) -> String
```

| 項目   | 説明                       |
| ------ | -------------------------- |
| 概要   | 40文字の16進数文字列に変換 |
| 戻り値 | 完全なハッシュ文字列       |

##### `Oid::short`

```rust
pub fn short(&self) -> String
```

| 項目   | 説明                      |
| ------ | ------------------------- |
| 概要   | 先頭7文字の短縮形式を取得 |
| 戻り値 | 7文字のハッシュ文字列     |

##### `Oid::as_bytes`

```rust
pub fn as_bytes(&self) -> &[u8; 20]
```

| 項目   | 説明                         |
| ------ | ---------------------------- |
| 概要   | 内部バイト配列への参照を取得 |
| 戻り値 | 20バイト配列への参照         |

#### トレイト実装

```rust
impl Display for Oid {
    // to_hex() と同じ出力
}
```

---

### 2.3 Object

Gitオブジェクトを表す列挙型。

```rust
#[derive(Debug, Clone)]
pub enum Object {
    Blob(Blob),
    Tree(Tree),
    Commit(Commit),
    Tag(Tag),
}
```

#### メソッド

##### `Object::kind`

```rust
pub fn kind(&self) -> &'static str
```

| 項目   | 説明                                               |
| ------ | -------------------------------------------------- |
| 概要   | オブジェクトの種類を文字列で取得                   |
| 戻り値 | `"blob"`, `"tree"`, `"commit"`, `"tag"` のいずれか |

##### `Object::as_blob`

```rust
pub fn as_blob(&self) -> Option<&Blob>
```

| 項目   | 説明                                          |
| ------ | --------------------------------------------- |
| 概要   | Blobとして取得を試みる                        |
| 戻り値 | `Some(&Blob)` - Blobの場合、`None` - それ以外 |

##### `Object::as_tree`

```rust
pub fn as_tree(&self) -> Option<&Tree>
```

##### `Object::as_commit`

```rust
pub fn as_commit(&self) -> Option<&Commit>
```

##### `Object::into_blob`

```rust
pub fn into_blob(self) -> Result<Blob>
```

| 項目   | 説明                                 |
| ------ | ------------------------------------ |
| 概要   | Blobに変換（所有権を移動）           |
| 戻り値 | `Ok(Blob)` - Blobの場合              |
| エラー | `Error::TypeMismatch` - Blobではない |

---

### 2.4 Blob

ファイル内容を表す構造体。

```rust
#[derive(Debug, Clone)]
pub struct Blob {
    content: Vec<u8>,
}
```

#### メソッド

##### `Blob::content`

```rust
pub fn content(&self) -> &[u8]
```

| 項目   | 説明                             |
| ------ | -------------------------------- |
| 概要   | ファイル内容をバイト列として取得 |
| 戻り値 | 内容への参照                     |

##### `Blob::content_str`

```rust
pub fn content_str(&self) -> Result<&str>
```

| 項目   | 説明                                   |
| ------ | -------------------------------------- |
| 概要   | ファイル内容をUTF-8文字列として取得    |
| 戻り値 | `Ok(&str)` - 有効なUTF-8の場合         |
| エラー | `Error::InvalidUtf8` - UTF-8として無効 |

##### `Blob::size`

```rust
pub fn size(&self) -> usize
```

| 項目   | 説明                 |
| ------ | -------------------- |
| 概要   | ファイルサイズを取得 |
| 戻り値 | バイト数             |

##### `Blob::is_binary`

```rust
pub fn is_binary(&self) -> bool
```

| 項目   | 説明                           |
| ------ | ------------------------------ |
| 概要   | バイナリファイルかどうかを推定 |
| 戻り値 | NULバイトを含む場合 `true`     |

---

### 2.5 Tree

ディレクトリ構造を表す構造体。

```rust
#[derive(Debug, Clone)]
pub struct Tree {
    entries: Vec<TreeEntry>,
}
```

#### メソッド

##### `Tree::entries`

```rust
pub fn entries(&self) -> &[TreeEntry]
```

| 項目   | 説明               |
| ------ | ------------------ |
| 概要   | エントリ一覧を取得 |
| 戻り値 | エントリのスライス |

##### `Tree::get`

```rust
pub fn get(&self, name: &str) -> Option<&TreeEntry>
```

| 項目   | 説明                                    |
| ------ | --------------------------------------- |
| 概要   | 名前でエントリを検索                    |
| 引数   | `name` - ファイル名またはディレクトリ名 |
| 戻り値 | `Some(&TreeEntry)` - 見つかった場合     |

##### `Tree::iter`

```rust
pub fn iter(&self) -> impl Iterator<Item = &TreeEntry>
```

---

### 2.6 TreeEntry

Treeの各エントリを表す構造体。

```rust
#[derive(Debug, Clone)]
pub struct TreeEntry {
    mode: FileMode,
    name: String,
    oid: Oid,
}
```

#### メソッド

##### `TreeEntry::mode`

```rust
pub fn mode(&self) -> FileMode
```

##### `TreeEntry::name`

```rust
pub fn name(&self) -> &str
```

##### `TreeEntry::oid`

```rust
pub fn oid(&self) -> &Oid
```

##### `TreeEntry::is_tree`

```rust
pub fn is_tree(&self) -> bool
```

##### `TreeEntry::is_blob`

```rust
pub fn is_blob(&self) -> bool
```

---

### 2.7 Commit

コミット情報を表す構造体。

```rust
#[derive(Debug, Clone)]
pub struct Commit {
    oid: Oid,
    tree: Oid,
    parents: Vec<Oid>,
    author: Signature,
    committer: Signature,
    message: String,
}
```

#### メソッド

##### `Commit::oid`

```rust
pub fn oid(&self) -> &Oid
```

| 項目 | 説明               |
| ---- | ------------------ |
| 概要 | コミットのIDを取得 |

##### `Commit::tree`

```rust
pub fn tree(&self) -> &Oid
```

| 項目 | 説明                         |
| ---- | ---------------------------- |
| 概要 | コミットが指すTreeのIDを取得 |

##### `Commit::parents`

```rust
pub fn parents(&self) -> &[Oid]
```

| 項目   | 説明                                              |
| ------ | ------------------------------------------------- |
| 概要   | 親コミットのID一覧を取得                          |
| 戻り値 | 通常1つ、マージコミットは2つ以上、初期コミットは0 |

##### `Commit::parent`

```rust
pub fn parent(&self) -> Option<&Oid>
```

| 項目   | 説明                                               |
| ------ | -------------------------------------------------- |
| 概要   | 最初の親コミットIDを取得                           |
| 戻り値 | `Some(&Oid)` - 親がある場合、`None` - 初期コミット |

##### `Commit::author`

```rust
pub fn author(&self) -> &Signature
```

##### `Commit::committer`

```rust
pub fn committer(&self) -> &Signature
```

##### `Commit::message`

```rust
pub fn message(&self) -> &str
```

| 項目 | 説明                         |
| ---- | ---------------------------- |
| 概要 | コミットメッセージ全体を取得 |

##### `Commit::summary`

```rust
pub fn summary(&self) -> &str
```

| 項目 | 説明                            |
| ---- | ------------------------------- |
| 概要 | コミットメッセージの1行目を取得 |

---

### 2.8 Signature

作成者/コミッター情報を表す構造体。

```rust
#[derive(Debug, Clone)]
pub struct Signature {
    name: String,
    email: String,
    time: i64,
    offset: i32,
}
```

#### コンストラクタ（Phase 2）

##### `Signature::new`

```rust
pub fn new(name: &str, email: &str) -> Signature
```

| 項目 | 説明                     |
| ---- | ------------------------ |
| 概要 | 現在時刻で署名を作成     |
| 引数 | `name` - 名前            |
| 引数 | `email` - メールアドレス |

##### `Signature::with_time`

```rust
pub fn with_time(name: &str, email: &str, time: i64, offset: i32) -> Signature
```

| 項目 | 説明                                                |
| ---- | --------------------------------------------------- |
| 概要 | 指定時刻で署名を作成                                |
| 引数 | `time` - Unixタイムスタンプ                         |
| 引数 | `offset` - UTCからの分オフセット（例: +0900 → 540） |

#### メソッド

##### `Signature::name`

```rust
pub fn name(&self) -> &str
```

##### `Signature::email`

```rust
pub fn email(&self) -> &str
```

##### `Signature::time`

```rust
pub fn time(&self) -> i64
```

| 項目 | 説明                     |
| ---- | ------------------------ |
| 概要 | Unixタイムスタンプを取得 |

##### `Signature::offset`

```rust
pub fn offset(&self) -> i32
```

| 項目 | 説明                               |
| ---- | ---------------------------------- |
| 概要 | タイムゾーンオフセット（分）を取得 |

---

### 2.9 Head

HEADの状態を表す列挙型。

```rust
#[derive(Debug, Clone)]
pub enum Head {
    /// ブランチを指している
    Branch(Branch),
    /// 直接コミットを指している（detached HEAD）
    Detached(Oid),
}
```

#### メソッド

##### `Head::oid`

```rust
pub fn oid(&self) -> &Oid
```

| 項目 | 説明                       |
| ---- | -------------------------- |
| 概要 | HEADが指すコミットIDを取得 |

##### `Head::is_detached`

```rust
pub fn is_detached(&self) -> bool
```

##### `Head::branch`

```rust
pub fn branch(&self) -> Option<&Branch>
```

| 項目   | 説明                                                |
| ------ | --------------------------------------------------- |
| 概要   | ブランチ情報を取得                                  |
| 戻り値 | `Some(&Branch)` - ブランチの場合、`None` - detached |

---

### 2.10 Branch

ブランチ情報を表す構造体。

```rust
#[derive(Debug, Clone)]
pub struct Branch {
    name: String,
    oid: Oid,
}
```

#### メソッド

##### `Branch::name`

```rust
pub fn name(&self) -> &str
```

##### `Branch::oid`

```rust
pub fn oid(&self) -> &Oid
```

##### `Branch::is_head`

```rust
pub fn is_head(&self) -> bool
```

| 項目 | 説明               |
| ---- | ------------------ |
| 概要 | 現在のHEADかどうか |

---

### 2.11 RemoteBranch（Phase 2.5）

リモートブランチ情報を表す構造体。

```rust
#[derive(Debug, Clone)]
pub struct RemoteBranch {
    remote: String,
    name: String,
    oid: Oid,
}
```

#### メソッド

##### `RemoteBranch::remote`

```rust
pub fn remote(&self) -> &str
```

| 項目 | 説明                            |
| ---- | ------------------------------- |
| 概要 | リモート名を取得（例: "origin"） |

##### `RemoteBranch::name`

```rust
pub fn name(&self) -> &str
```

| 項目 | 説明                          |
| ---- | ----------------------------- |
| 概要 | ブランチ名を取得（例: "main"） |

##### `RemoteBranch::full_name`

```rust
pub fn full_name(&self) -> String
```

| 項目 | 説明                                   |
| ---- | -------------------------------------- |
| 概要 | 完全名を取得（例: "origin/main"）      |

##### `RemoteBranch::oid`

```rust
pub fn oid(&self) -> &Oid
```

---

### 2.12 Tag（Phase 2.5）

タグ情報を表す構造体。軽量タグと注釈付きタグの両方をサポート。

```rust
#[derive(Debug, Clone)]
pub struct Tag {
    name: String,
    target: Oid,
    message: Option<String>,
    tagger: Option<Signature>,
}
```

#### メソッド

##### `Tag::name`

```rust
pub fn name(&self) -> &str
```

##### `Tag::target`

```rust
pub fn target(&self) -> &Oid
```

| 項目 | 説明                     |
| ---- | ------------------------ |
| 概要 | タグが指すオブジェクトID |

##### `Tag::message`

```rust
pub fn message(&self) -> Option<&str>
```

| 項目   | 説明                                       |
| ------ | ------------------------------------------ |
| 概要   | 注釈付きタグのメッセージを取得             |
| 戻り値 | `Some(&str)` - 注釈付きタグ、`None` - 軽量タグ |

##### `Tag::tagger`

```rust
pub fn tagger(&self) -> Option<&Signature>
```

| 項目   | 説明                                     |
| ------ | ---------------------------------------- |
| 概要   | 注釈付きタグの作成者を取得               |
| 戻り値 | `Some(&Signature)` - 注釈付きタグの場合   |

##### `Tag::is_annotated`

```rust
pub fn is_annotated(&self) -> bool
```

| 項目 | 説明                       |
| ---- | -------------------------- |
| 概要 | 注釈付きタグかどうかを判定 |

---

### 2.13 StatusEntry

ステータスのエントリを表す構造体。

```rust
#[derive(Debug, Clone)]
pub struct StatusEntry {
    path: PathBuf,
    status: FileStatus,
}
```

#### メソッド

##### `StatusEntry::path`

```rust
pub fn path(&self) -> &Path
```

##### `StatusEntry::status`

```rust
pub fn status(&self) -> FileStatus
```

---

### 2.12 FileStatus

ファイルの状態を表す列挙型。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    /// Git管理外
    Untracked,
    /// 変更あり（未ステージ）
    Modified,
    /// ステージ済み（新規）
    Added,
    /// ステージ済み（変更）
    StagedModified,
    /// ステージ済み（削除）
    StagedDeleted,
    /// 削除された（未ステージ）
    Deleted,
    /// 名前変更
    Renamed,
}
```

---

### 2.13 Index

インデックス（ステージングエリア）を表す構造体。

```rust
#[derive(Debug, Clone)]
pub struct Index {
    version: u32,
    entries: Vec<IndexEntry>,
}
```

#### メソッド

##### `Index::version`

```rust
pub fn version(&self) -> u32
```

##### `Index::entries`

```rust
pub fn entries(&self) -> &[IndexEntry]
```

##### `Index::get`

```rust
pub fn get(&self, path: &Path) -> Option<&IndexEntry>
```

コンフリクト中のパスでは、最も小さいstageのエントリを返す。

##### `Index::get_stage` / `Index::has_conflicts` / `Index::conflicted_paths`

```rust
pub fn get_stage(&self, path: &Path, stage: u8) -> Option<&IndexEntry>
pub fn has_conflicts(&self) -> bool
pub fn conflicted_paths(&self) -> Vec<PathBuf>
```

##### `Index::add` / `Index::remove`

エントリはGitと同じ順（パスのバイト列、次にstage）に保つ。stage 0の`add`は同じパスの全stageを置き換え（コンフリクトの解消）、`remove`は全stageを削除する。

##### `Index::len`

```rust
pub fn len(&self) -> usize
```

##### `Index::is_empty`

```rust
pub fn is_empty(&self) -> bool
```

---

### 2.14 IndexEntry

インデックスのエントリを表す構造体。

```rust
#[derive(Debug, Clone)]
pub struct IndexEntry {
    oid: Oid,
    path: PathBuf,
    mode: FileMode,
    size: u32,
    mtime: u64,
    ctime: u64,
}
```

#### メソッド

##### `IndexEntry::oid`

```rust
pub fn oid(&self) -> &Oid
```

##### `IndexEntry::path`

```rust
pub fn path(&self) -> &Path
```

##### `IndexEntry::mode`

```rust
pub fn mode(&self) -> FileMode
```

##### `IndexEntry::size`

```rust
pub fn size(&self) -> u32
```

---

### 2.15 FileMode

ファイルモードを表す列挙型。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    /// 通常ファイル (100644)
    Regular,
    /// 実行可能ファイル (100755)
    Executable,
    /// シンボリックリンク (120000)
    Symlink,
    /// サブモジュール (160000)
    Submodule,
    /// ディレクトリ (040000)
    Tree,
}
```

#### メソッド

##### `FileMode::as_u32`

```rust
pub fn as_u32(&self) -> u32
```

| 項目   | 説明                                            |
| ------ | ----------------------------------------------- |
| 概要   | Gitの数値モードを取得                           |
| 戻り値 | `100644`, `100755`, `120000`, `160000`, `40000` |

---

### 2.16 LogIterator

コミット履歴のイテレータ。

```rust
pub struct LogIterator<'a> { /* private fields */ }
```

#### トレイト実装

```rust
impl<'a> Iterator for LogIterator<'a> {
    type Item = Result<Commit>;
}
```

| 項目   | 説明                              |
| ------ | --------------------------------- |
| 概要   | 親コミットを辿りながら遅延取得    |
| 戻り値 | `Some(Ok(Commit))` - 次のコミット |
| 戻り値 | `Some(Err(e))` - 読み取りエラー   |
| 戻り値 | `None` - 履歴の終端               |

---

### 2.17 LogOptions（Phase 2.5）

ログ取得オプションのビルダー。

```rust
#[derive(Debug, Clone, Default)]
pub struct LogOptions {
    paths: Vec<PathBuf>,
    max_count: Option<usize>,
    since: Option<i64>,
    until: Option<i64>,
    first_parent: bool,
    author: Option<String>,
    from: Option<Oid>,
}
```

#### コンストラクタ

##### `LogOptions::new`

```rust
pub fn new() -> Self
```

#### ビルダーメソッド

##### `LogOptions::path`

```rust
pub fn path<P: AsRef<Path>>(self, path: P) -> Self
```

| 項目 | 説明                               |
| ---- | ---------------------------------- |
| 概要 | 特定パスの変更を含むコミットのみ   |

##### `LogOptions::paths`

```rust
pub fn paths<I, P>(self, paths: I) -> Self
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
```

| 項目 | 説明                     |
| ---- | ------------------------ |
| 概要 | 複数パスを一度に指定     |

##### `LogOptions::max_count`

```rust
pub fn max_count(self, n: usize) -> Self
```

| 項目 | 説明                 |
| ---- | -------------------- |
| 概要 | 最大取得件数を指定   |

##### `LogOptions::since`

```rust
pub fn since(self, date: &str) -> Self
```

| 項目 | 説明                               |
| ---- | ---------------------------------- |
| 概要 | この日時以降のコミットのみ         |
| 引数 | `date` - "YYYY-MM-DD"形式の日付    |

##### `LogOptions::until`

```rust
pub fn until(self, date: &str) -> Self
```

| 項目 | 説明                           |
| ---- | ------------------------------ |
| 概要 | この日時以前のコミットのみ     |

##### `LogOptions::first_parent`

```rust
pub fn first_parent(self, enabled: bool) -> Self
```

| 項目 | 説明                                 |
| ---- | ------------------------------------ |
| 概要 | マージコミットの最初の親のみを辿る   |

##### `LogOptions::author`

```rust
pub fn author(self, name: &str) -> Self
```

| 項目 | 説明                         |
| ---- | ---------------------------- |
| 概要 | 作者名でフィルタ（部分一致） |

##### `LogOptions::from`

```rust
pub fn from(self, oid: Oid) -> Self
```

| 項目 | 説明                               |
| ---- | ---------------------------------- |
| 概要 | 開始コミットを指定（デフォルトHEAD） |

---

### 2.18 TreeDiff（Phase 2.5）

Tree間の差分を表す構造体。

```rust
#[derive(Debug, Clone)]
pub struct TreeDiff {
    deltas: Vec<DiffDelta>,
}
```

#### メソッド

##### `TreeDiff::deltas`

```rust
pub fn deltas(&self) -> &[DiffDelta]
```

| 項目 | 説明             |
| ---- | ---------------- |
| 概要 | 差分エントリ一覧 |

##### `TreeDiff::stats`

```rust
pub fn stats(&self) -> DiffStats
```

| 項目 | 説明                       |
| ---- | -------------------------- |
| 概要 | 差分の統計情報を取得       |

##### `TreeDiff::is_empty`

```rust
pub fn is_empty(&self) -> bool
```

| 項目 | 説明                   |
| ---- | ---------------------- |
| 概要 | 差分がないかどうか     |

#### トレイト実装

```rust
impl IntoIterator for TreeDiff { /* ... */ }
impl<'a> IntoIterator for &'a TreeDiff { /* ... */ }
```

---

### 2.19 DiffDelta（Phase 2.5）

差分の各エントリを表す構造体。

```rust
#[derive(Debug, Clone)]
pub struct DiffDelta {
    status: DiffStatus,
    path: PathBuf,
    old_path: Option<PathBuf>,
    old_oid: Option<Oid>,
    new_oid: Option<Oid>,
    old_mode: Option<FileMode>,
    new_mode: Option<FileMode>,
}
```

#### メソッド

##### `DiffDelta::status`

```rust
pub fn status(&self) -> DiffStatus
```

##### `DiffDelta::path`

```rust
pub fn path(&self) -> &Path
```

| 項目 | 説明                                       |
| ---- | ------------------------------------------ |
| 概要 | ファイルパス（新しい方、またはリネーム後） |

##### `DiffDelta::old_path`

```rust
pub fn old_path(&self) -> Option<&Path>
```

| 項目 | 説明                         |
| ---- | ---------------------------- |
| 概要 | リネーム/コピー元のパス      |

##### `DiffDelta::old_oid`

```rust
pub fn old_oid(&self) -> Option<&Oid>
```

##### `DiffDelta::new_oid`

```rust
pub fn new_oid(&self) -> Option<&Oid>
```

##### `DiffDelta::status_char`

```rust
pub fn status_char(&self) -> char
```

| 項目   | 説明                                     |
| ------ | ---------------------------------------- |
| 概要   | git status形式の1文字ステータス          |
| 戻り値 | `'A'`, `'D'`, `'M'`, `'R'`, `'C'` のいずれか |

---

### 2.20 DiffStatus（Phase 2.5）

差分のステータスを表す列挙型。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffStatus {
    /// 新規追加
    Added,
    /// 削除
    Deleted,
    /// 変更
    Modified,
    /// リネーム
    Renamed,
    /// コピー
    Copied,
}
```

---

### 2.21 DiffStats（Phase 2.5）

差分の統計情報を表す構造体。

```rust
#[derive(Debug, Clone, Default)]
pub struct DiffStats {
    pub added: usize,
    pub deleted: usize,
    pub modified: usize,
    pub renamed: usize,
    pub copied: usize,
}
```

---

### 2.22 Config（Phase 2.5）

Git設定を表す構造体。

```rust
#[derive(Debug, Clone)]
pub struct Config {
    entries: HashMap<String, String>,
}
```

#### メソッド

##### `Config::get`

```rust
pub fn get(&self, key: &str) -> Option<&str>
```

| 項目   | 説明                                       |
| ------ | ------------------------------------------ |
| 概要   | 設定値を取得                               |
| 引数   | `key` - "section.key"形式（例: "user.name"） |
| 戻り値 | `Some(&str)` - 値が存在する場合            |

##### `Config::get_or`

```rust
pub fn get_or(&self, key: &str, default: &str) -> &str
```

| 項目   | 説明                                 |
| ------ | ------------------------------------ |
| 概要   | 設定値を取得、なければデフォルト値   |

---

### 2.23 Error

エラー型。

```rust
#[derive(Debug)]
pub enum Error {
    /// ファイルI/Oエラー
    Io(std::io::Error),
    
    /// 有効なGitリポジトリではない
    NotARepository(PathBuf),
    
    /// オブジェクトが見つからない
    ObjectNotFound(Oid),
    
    /// 参照が見つからない
    RefNotFound(String),
    
    /// パスが見つからない
    PathNotFound(PathBuf),
    
    /// 不正なオブジェクトID
    InvalidOid(String),
    
    /// 不正な参照名
    InvalidRefName(String),
    
    /// 不正なオブジェクト形式
    InvalidObject { oid: Oid, reason: String },
    
    /// 不正なインデックス形式
    InvalidIndex { version: u32, reason: String },
    
    /// 型の不一致
    TypeMismatch { expected: &'static str, actual: &'static str },
    
    /// UTF-8変換エラー
    InvalidUtf8,
    
    /// 解凍エラー
    DecompressionFailed,
    
    /// 参照が既に存在（Phase 2）
    RefAlreadyExists(String),
    
    /// 現在のブランチは削除不可（Phase 2）
    CannotDeleteCurrentBranch,
    
    /// 空のコミット（Phase 2）
    EmptyCommit,
    
    /// 未コミットの変更あり（Phase 2）
    DirtyWorkingTree,
    
    /// 設定が見つからない（Phase 2）
    ConfigNotFound(String),

    /// 既にリポジトリが存在（Phase 2.5）
    AlreadyARepository(PathBuf),

    /// packed-refsの不正な行（Phase 3）
    InvalidPackedRefs { line: usize, reason: String },

    /// packed参照の削除は未対応（Phase 3）
    PackedRefDeletionUnsupported(String),

    /// 不正・破損したpack index（Phase 3）
    InvalidPackIndex { reason: String },

    /// 未対応のpack indexバージョン（v1等）（Phase 3）
    UnsupportedPackIndexVersion(u32),

    /// 不正・破損したpack、または適用できないdelta（Phase 3）
    InvalidPack { reason: String },

    /// 未対応のpackバージョン（Phase 3）
    UnsupportedPackVersion(u32),

    /// 未対応のリポジトリ形式（SHA-256、reftable等）（Phase 3）
    UnsupportedRepositoryFormat(String),

    /// pack読み取りのサイズ・delta深度の上限超過（Phase 3）
    PackLimitExceeded { reason: String },

    /// indexに未解消のコンフリクトがある（commit・checkoutを拒否）
    UnmergedPaths(Vec<PathBuf>),

    /// 未追跡で無視指定に一致するパス（addを拒否）
    IgnoredPath(PathBuf),
}
```

`ObjectNotFound`は「どこにも存在しない」場合のみで、破損や未対応は上記の個別エラーとなる。行差分の非テキスト・上限超過はエラーではなく`BlobDiffContent`で表す（2.24）。

#### トレイト実装

```rust
impl std::fmt::Display for Error { /* ... */ }
impl std::error::Error for Error { /* ... */ }
impl From<std::io::Error> for Error { /* ... */ }
```

### 2.24 BlobDiff / DiffHunk / DiffLine / DiffOptions（Phase 3）

2つのBlobの行差分。`TreeDiff`は変更ファイル一覧として維持し、本文の比較はこの型で行う。

```rust
impl Repository {
    /// None は片側不在（追加・削除）。Blobでない・読めない場合はエラー
    pub fn diff_blobs(&self, old: Option<&Oid>, new: Option<&Oid>, options: &DiffOptions)
        -> Result<BlobDiff>;
}

impl BlobDiff {
    pub fn compute(old: Option<&[u8]>, new: Option<&[u8]>, options: &DiffOptions) -> BlobDiff;
    pub fn old_exists(&self) -> bool;       // 片側不在と実在する空Blobを区別
    pub fn new_exists(&self) -> bool;
    pub fn old_size(&self) -> usize;
    pub fn new_size(&self) -> usize;
    pub fn is_identical(&self) -> bool;     // bytes比較。NonText/Skippedでも正確
    pub fn content(&self) -> &BlobDiffContent;
    pub fn hunks(&self) -> Option<&[DiffHunk]>;   // Text以外はNone
    pub fn lines_added(&self) -> Option<usize>;
    pub fn lines_removed(&self) -> Option<usize>;
}

pub enum BlobDiffContent {
    Text(Vec<DiffHunk>),        // 完全な行差分。同一内容なら空
    NonText(NonTextReason),     // ContainsNul / InvalidUtf8（暗黙置換しない）
    Skipped(SkipReason),        // InputTooLarge { limit } / TooComplex { limit }
}

impl DiffHunk {
    pub fn old_start(&self) -> usize;   // 1始まり。範囲が空なら直前の行番号（先頭は0）
    pub fn old_lines(&self) -> usize;
    pub fn new_start(&self) -> usize;
    pub fn new_lines(&self) -> usize;
    pub fn lines(&self) -> &[DiffLine]; // 変更内では削除が追加より先
    pub fn header(&self) -> String;     // "@@ -1,3 +1,4 @@"
}

impl DiffLine {
    pub fn kind(&self) -> LineKind;               // Context / Added / Removed
    pub fn old_lineno(&self) -> Option<usize>;    // 追加行はNone
    pub fn new_lineno(&self) -> Option<usize>;    // 削除行はNone
    pub fn content(&self) -> &str;                // 改行込み
    pub fn text(&self) -> &str;                   // 改行（\n / \r\n）を除く
    pub fn ending(&self) -> LineEnding;           // Lf / CrLf / None
}

impl DiffOptions {
    pub fn new() -> Self;                         // 文脈3行、8 MiB、5,000万ステップ
    pub fn context_lines(self, lines: usize) -> Self;
    pub fn max_input_size(self, bytes: usize) -> Self;
    pub fn max_cost(self, steps: u64) -> Self;
}
```

判定順はサイズ上限 → NUL → UTF-8 → 行差分（計算量上限）。同じ入力とオプションでは常に同じ結果になる。既定値の根拠はREADMEの「処理上限の既定値」を参照。

### 2.25 RenameOptions / RenameDetection / RenameLimit（Phase 3、任意）

```rust
impl Repository {
    pub fn diff_trees_with_options(&self, old: Option<&Tree>, new: &Tree, options: &RenameOptions)
        -> Result<TreeDiff>;
    pub fn commit_diff_with_options(&self, commit: &Commit, options: &RenameOptions)
        -> Result<TreeDiff>;
}

pub enum RenameDetection { Off, Exact /* 既定 */, Similar }

impl RenameOptions {
    pub fn new() -> Self;                              // Exact、50%、10万ペア、1 MiB
    pub fn detection(self, detection: RenameDetection) -> Self;
    pub fn threshold(self, percent: u8) -> Self;       // 0..=100
    pub fn max_pairs(self, pairs: usize) -> Self;      // 削除数×追加数の上限
    pub fn max_file_size(self, bytes: usize) -> Self;
}

pub enum RenameLimit {
    TooManyPairs { pairs: usize, limit: usize },       // 類似度検出を行わなかった
    FileTooLarge { files: usize, limit: usize },       // 一部のファイルを比較しなかった
}

impl TreeDiff { pub fn rename_limits(&self) -> &[RenameLimit]; }   // 空なら検出は完全
impl DiffDelta { pub fn similarity(&self) -> Option<u8>; }        // 完全一致は100
```

- `diff_trees`/`commit_diff`は従来どおり完全一致のみ（`RenameOptions::default()`と同じ結果）。
- 類似度は、`\n`の直後で区切った行を多重集合として比べ、共通する行のバイト数を大きい方のファイルサイズで割った値（%、切り捨て）。行の順序は問わない。
- 対象は通常ファイルと実行ファイルだけ。空ファイル、NULを含むファイル、サイズ上限を超えるファイルは類似度では対応付けない（完全一致の対応付けは従来どおり行う）。
- 完全一致の対応付けの後に、残った削除と追加の全組を比較する。しきい値以上の組を、類似度の降順 → ファイル名が同じもの → 新パス → 旧パスの順に、一対一で貪欲に選ぶ。入力の順序に依存しない。
- 対応付かなかったもの、上限で比較しなかったものは、追加・削除のまま残る。

---

## 3. 使用例

### 3.1 リポジトリを開いてログを表示

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    // カレントディレクトリから.gitを探索
    let repo = Repository::discover(".")?;
    
    // 最新10件のコミットを表示
    for commit in repo.log()?.take(10) {
        let commit = commit?;
        println!("{} {}", commit.oid().short(), commit.summary());
    }
    
    Ok(())
}
```

### 3.2 ステータスを表示

```rust
use zerogit::{Repository, FileStatus, Result};

fn main() -> Result<()> {
    let repo = Repository::open(".")?;
    
    for entry in repo.status()? {
        let status_char = match entry.status() {
            FileStatus::Untracked => '?',
            FileStatus::Modified => 'M',
            FileStatus::Added => 'A',
            FileStatus::Deleted => 'D',
            FileStatus::StagedModified => 'M',
            FileStatus::StagedDeleted => 'D',
            FileStatus::Renamed => 'R',
        };
        println!("{} {}", status_char, entry.path().display());
    }
    
    Ok(())
}
```

### 3.3 特定コミットの詳細を表示

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;
    
    // 短縮形式でもOK
    let commit = repo.commit("abc1234")?;
    
    println!("Commit: {}", commit.oid());
    println!("Author: {} <{}>", commit.author().name(), commit.author().email());
    println!("Date:   {}", commit.author().time());
    println!();
    println!("{}", commit.message());
    
    Ok(())
}
```

### 3.4 Treeの内容を走査

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;
    
    // HEADコミットのTreeを取得
    let head = repo.head()?;
    let commit = repo.commit(&head.oid().to_hex())?;
    let tree = repo.tree(&commit.tree().to_hex())?;
    
    for entry in tree.entries() {
        let kind = if entry.is_tree() { "tree" } else { "blob" };
        println!("{} {} {}", entry.oid().short(), kind, entry.name());
    }
    
    Ok(())
}
```

### 3.5 ブランチ一覧を表示

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;
    let head = repo.head()?;
    
    for branch in repo.branches()? {
        let marker = if head.branch().map(|b| b.name()) == Some(branch.name()) {
            "* "
        } else {
            "  "
        };
        println!("{}{}", marker, branch.name());
    }
    
    Ok(())
}
```

### 3.6 ファイルの内容を取得

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;
    
    // HEAD時点のREADME.mdを取得
    let head = repo.head()?;
    let commit = repo.commit(&head.oid().to_hex())?;
    let tree = repo.tree(&commit.tree().to_hex())?;
    
    if let Some(entry) = tree.get("README.md") {
        let blob = repo.blob(&entry.oid().to_hex())?;
        if let Ok(content) = blob.content_str() {
            println!("{}", content);
        }
    }
    
    Ok(())
}
```

### 3.7 コミットの作成（Phase 2）

```rust
use zerogit::{Repository, Signature, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;
    
    // ファイルをステージ
    repo.add("src/main.rs")?;
    repo.add("README.md")?;
    
    // 署名を作成
    let author = Signature::new("John Doe", "john@example.com");
    
    // コミット
    let oid = repo.create_commit("Add new feature", Some(&author), None)?;
    println!("Created commit: {}", oid);
    
    Ok(())
}
```

### 3.8 エラーハンドリング

```rust
use zerogit::{Repository, Error, Result};

fn main() {
    match run() {
        Ok(()) => {}
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

fn run() -> Result<()> {
    let repo = Repository::discover(".")?;

    match repo.commit("nonexistent") {
        Ok(commit) => println!("{}", commit.summary()),
        Err(Error::ObjectNotFound(oid)) => {
            eprintln!("Commit {} not found", oid);
        }
        Err(Error::InvalidOid(s)) => {
            eprintln!("Invalid commit ID: {}", s);
        }
        Err(e) => return Err(e),
    }

    Ok(())
}
```

### 3.9 リポジトリの初期化（Phase 2.5）

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    // 新しいGitリポジトリを初期化
    let repo = Repository::init("./my-project")?;

    println!("Initialized empty Git repository in {}", repo.git_dir().display());
    Ok(())
}
```

### 3.10 リモートブランチとタグ一覧（Phase 2.5）

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;

    // リモートブランチ一覧
    println!("Remote branches:");
    for rb in repo.remote_branches()? {
        println!("  {}/{}", rb.remote(), rb.name());
    }

    // タグ一覧
    println!("\nTags:");
    for tag in repo.tags()? {
        println!("  {} -> {}", tag.name(), tag.target().short());
        if let Some(message) = tag.message() {
            println!("    {}", message);
        }
    }

    Ok(())
}
```

### 3.11 ログフィルタリング（Phase 2.5）

```rust
use zerogit::{Repository, LogOptions, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;

    // 特定ファイルの変更履歴を最大10件取得
    let log = repo.log_with_options(
        LogOptions::new()
            .path("src/main.rs")
            .max_count(10)
    )?;

    for commit in log {
        let commit = commit?;
        println!("{} {}", commit.oid().short(), commit.summary());
    }

    Ok(())
}
```

### 3.12 Tree Diff（Phase 2.5）

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;

    // 直近のコミットの変更ファイル一覧
    let head = repo.head()?;
    let commit = repo.commit(&head.oid().to_hex())?;
    let diff = repo.commit_diff(&commit)?;

    println!("Changes in {}:", commit.oid().short());
    for delta in diff.deltas() {
        println!("  {} {}", delta.status_char(), delta.path().display());
    }

    let stats = diff.stats();
    println!("\n{} added, {} deleted, {} modified",
        stats.added, stats.deleted, stats.modified);

    Ok(())
}
```

### 3.13 Working Tree Diff（Phase 2.5）

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;

    // 未ステージの変更（git diff 相当）
    let unstaged = repo.diff_index_to_workdir()?;
    if !unstaged.is_empty() {
        println!("Unstaged changes:");
        for delta in unstaged.deltas() {
            println!("  {} {}", delta.status_char(), delta.path().display());
        }
    }

    // ステージ済みの変更（git diff --staged 相当）
    let staged = repo.diff_head_to_index()?;
    if !staged.is_empty() {
        println!("\nStaged changes:");
        for delta in staged.deltas() {
            println!("  {} {}", delta.status_char(), delta.path().display());
        }
    }

    Ok(())
}
```

### 3.14 Git設定の読み取り（Phase 2.5）

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;
    let config = repo.config()?;

    let name = config.get("user.name").unwrap_or("Unknown");
    let email = config.get("user.email").unwrap_or("unknown@example.com");

    println!("Author: {} <{}>", name, email);

    Ok(())
}
```

### 3.15 文書差分フロー（Phase 3）

KazeNhanh等で想定する、コミット確定 → 変更一覧 → 選択ファイルの行差分の流れ。完全なコードは`examples/document_diff.rs`、受け入れテストは`tests/document_flow_test.rs`。

1. 比較開始時にコミットをOIDへ解決して固定する（`repo.head()?.oid()`や`resolve_short_oid`）。以降はOIDのみで読むため、参照の更新や`git gc`の影響を受けない。
2. 比較元を決める。既定は第一親（`commit_diff`）、初回コミットは空Tree（`diff_trees(None, &tree)`）、マージの別の親は`diff_trees(Some(&parent_tree), &tree)`で明示する。
3. 変更一覧は`diff_trees`の結果のみで作り、Blobは読まない。`DiffDelta`のstatus・旧新パス・旧新mode・旧新OIDを表示する。
4. 詳細表示時に`diff_blobs(delta.old_oid(), delta.new_oid(), &options)`を呼ぶ。gitlink（`FileMode::Submodule`）はこのリポジトリのBlobではないため読まずにコミットOIDの変化として扱う。symlinkはリンク先文字列の差分になる。
5. `BlobDiffContent::NonText`/`Skipped`と`Err`は、それぞれ「テキストでない」「省略」「エラー」として表示し、空の差分として扱わない。
