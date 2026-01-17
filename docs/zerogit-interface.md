# zerogit インターフェース設計書

## 1. 公開API一覧

### 1.1 モジュール構成

```rust
pub mod zerogit {
    // コアAPI
    pub struct Repository;
    pub struct Oid;
    
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
    
    // ステータス
    pub struct StatusEntry;
    pub enum FileStatus;
    
    // インデックス
    pub struct Index;
    pub struct IndexEntry;
    
    // エラー
    pub enum Error;
    pub type Result<T> = std::result::Result<T, Error>;
    
    // 定数
    pub enum FileMode;
}
```

### 1.2 公開要素サマリー

| カテゴリ     | 名前          | 種別   | Phase |
| ------------ | ------------- | ------ | ----- |
| コア         | `Repository`  | struct | 1     |
| コア         | `Oid`         | struct | 1     |
| オブジェクト | `Object`      | enum   | 1     |
| オブジェクト | `Blob`        | struct | 1     |
| オブジェクト | `Tree`        | struct | 1     |
| オブジェクト | `TreeEntry`   | struct | 1     |
| オブジェクト | `Commit`      | struct | 1     |
| オブジェクト | `Signature`   | struct | 1     |
| 参照         | `Head`        | enum   | 1     |
| 参照         | `Branch`      | struct | 1     |
| ステータス   | `StatusEntry` | struct | 1     |
| ステータス   | `FileStatus`  | enum   | 1     |
| インデックス | `Index`       | struct | 1     |
| インデックス | `IndexEntry`  | struct | 1     |
| エラー       | `Error`       | enum   | 1     |
| 定数         | `FileMode`    | enum   | 1     |

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
| エラー | `Error::PathNotFound` - ファイルが存在しない                        |
| エラー | `Error::Io` - ファイル読み取りエラー                                |

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

### 2.11 StatusEntry

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

### 2.17 Error

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
}
```

#### トレイト実装

```rust
impl std::fmt::Display for Error { /* ... */ }
impl std::error::Error for Error { /* ... */ }
impl From<std::io::Error> for Error { /* ... */ }
```

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
