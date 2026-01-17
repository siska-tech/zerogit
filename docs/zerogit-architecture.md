# zerogit アーキテクチャ設計書

## 1. システム構成図

### 1.1 レイヤー構成

```
┌─────────────────────────────────────────────────────────────┐
│                      Public API Layer                       │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐           │
│  │ Repository  │ │   Status    │ │     Log     │           │
│  └──────┬──────┘ └──────┬──────┘ └──────┬──────┘           │
└─────────┼───────────────┼───────────────┼───────────────────┘
          │               │               │
┌─────────▼───────────────▼───────────────▼───────────────────┐
│                      Domain Layer                           │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐           │
│  │   Objects   │ │    Refs     │ │    Index    │           │
│  │ (blob/tree/ │ │  (HEAD/     │ │  (.git/     │           │
│  │  commit/tag)│ │   branches) │ │   index)    │           │
│  └──────┬──────┘ └──────┬──────┘ └──────┬──────┘           │
└─────────┼───────────────┼───────────────┼───────────────────┘
          │               │               │
┌─────────▼───────────────▼───────────────▼───────────────────┐
│                   Infrastructure Layer                      │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐           │
│  │    Hash     │ │ Compression │ │  FileSystem │           │
│  │   (SHA-1)   │ │   (zlib)    │ │    (I/O)    │           │
│  └─────────────┘ └─────────────┘ └─────────────┘           │
└─────────────────────────────────────────────────────────────┘
```

### 1.2 モジュール一覧と責務

| レイヤー       | モジュール    | 責務                                                  |
| -------------- | ------------- | ----------------------------------------------------- |
| Public API     | `repository`  | リポジトリ操作の統合エントリーポイント                |
| Public API     | `status`      | ワーキングツリー状態の取得                            |
| Public API     | `log`         | コミット履歴の取得・走査                              |
| Domain         | `objects`     | Gitオブジェクト（blob/tree/commit/tag）のパースと生成 |
| Domain         | `refs`        | 参照（HEAD/branches/tags）の解決と管理                |
| Domain         | `index`       | ステージング領域（.git/index）の読み書き              |
| Infrastructure | `hash`        | SHA-1ハッシュ計算（自前実装）                         |
| Infrastructure | `compression` | zlib圧縮・解凍（miniz_oxide wrapper）                 |
| Infrastructure | `fs`          | ファイルシステム操作の抽象化                          |

### 1.3 ディレクトリ構造

```
zerogit/
├── Cargo.toml
├── src/
│   ├── lib.rs                 # クレートルート、pub use
│   │
│   ├── repository.rs          # Repository構造体
│   ├── status.rs              # Status機能
│   ├── log.rs                 # Log機能
│   ├── error.rs               # エラー型定義
│   │
│   ├── objects/
│   │   ├── mod.rs
│   │   ├── oid.rs             # オブジェクトID (SHA-1)
│   │   ├── blob.rs            # Blobオブジェクト
│   │   ├── tree.rs            # Treeオブジェクト
│   │   ├── commit.rs          # Commitオブジェクト
│   │   ├── tag.rs             # Tagオブジェクト
│   │   └── store.rs           # オブジェクトストア（読み書き）
│   │
│   ├── refs/
│   │   ├── mod.rs
│   │   ├── head.rs            # HEAD解決
│   │   ├── branch.rs          # ブランチ操作
│   │   └── resolver.rs        # symbolic-ref解決
│   │
│   ├── index/
│   │   ├── mod.rs
│   │   ├── entry.rs           # Indexエントリ
│   │   ├── reader.rs          # Index読み取り
│   │   └── writer.rs          # Index書き込み（Phase 2）
│   │
│   └── infra/
│       ├── mod.rs
│       ├── hash.rs            # SHA-1実装
│       ├── compression.rs     # zlib wrapper
│       └── fs.rs              # ファイルシステム抽象
│
└── tests/
    ├── integration/           # 統合テスト
    └── fixtures/              # テスト用リポジトリ
```

---

## 2. モジュール間の関係

### 2.1 依存関係図

```
                    ┌──────────────┐
                    │  repository  │
                    └──────┬───────┘
           ┌───────────────┼───────────────┐
           ▼               ▼               ▼
    ┌──────────┐    ┌──────────┐    ┌──────────┐
    │  status  │    │   log    │    │   refs   │
    └────┬─────┘    └────┬─────┘    └────┬─────┘
         │               │               │
         ▼               ▼               │
    ┌──────────┐    ┌──────────┐         │
    │  index   │    │ objects  │◄────────┘
    └────┬─────┘    └────┬─────┘
         │               │
         └───────┬───────┘
                 ▼
    ┌────────────────────────────┐
    │          infra             │
    │  (hash, compression, fs)   │
    └────────────────────────────┘
```

**依存ルール:**
- 上位レイヤーは下位レイヤーに依存可
- 同一レイヤー内の横断的依存は最小化
- `infra` は他モジュールに依存しない

### 2.2 データフロー

#### オブジェクト読み取りフロー

```
[.git/objects/ab/cdef...]
        │
        ▼ fs::read_file()
    ┌─────────┐
    │ 圧縮    │
    │ データ  │
    └────┬────┘
         │ compression::decompress()
         ▼
    ┌─────────┐
    │ 生      │  "blob 123\0<content>"
    │ データ  │
    └────┬────┘
         │ objects::parse()
         ▼
    ┌─────────┐
    │ Object  │  Blob { content: Vec<u8> }
    │ 構造体  │
    └─────────┘
```

#### status() 処理フロー

```
    ┌─────────────┐
    │ Repository  │
    └──────┬──────┘
           │ status()
           ▼
    ┌──────────────────────────────────────┐
    │              Status                   │
    │  1. index.read()     → IndexEntries  │
    │  2. refs.head()      → HEAD commit   │
    │  3. objects.tree()   → HEAD tree     │
    │  4. fs.walk()        → WorkingTree   │
    │  5. diff(index, tree, working)       │
    └──────────────────────────────────────┘
           │
           ▼
    Vec<StatusEntry>
```

### 2.3 シーケンス図

#### ユースケース: `Repository::open()` → `log()`

```
User        Repository      Refs        Objects     Compression
 │               │            │            │             │
 │ open(path)    │            │            │             │
 │──────────────>│            │            │             │
 │               │ validate   │            │             │
 │               │ .git dir   │            │             │
 │               │            │            │             │
 │ log()         │            │            │             │
 │──────────────>│            │            │             │
 │               │ head()     │            │             │
 │               │───────────>│            │             │
 │               │ <─ sha ────│            │             │
 │               │            │            │             │
 │               │ read_commit(sha)        │             │
 │               │────────────────────────>│             │
 │               │            │            │ decompress  │
 │               │            │            │────────────>│
 │               │            │            │<────────────│
 │               │            │            │ parse       │
 │               │<─ Commit ──────────────│             │
 │               │            │            │             │
 │               │ [repeat for parents]    │             │
 │<─ Iterator ───│            │            │             │
 │               │            │            │             │
```

#### ユースケース: `add()` → `commit()` (Phase 2)

```
User        Repository      Index       Objects      Refs
 │               │            │            │           │
 │ add(path)     │            │            │           │
 │──────────────>│            │            │           │
 │               │ read_file  │            │           │
 │               │ hash_blob  │            │           │
 │               │────────────────────────>│           │
 │               │<─ oid ─────────────────│           │
 │               │ write_blob │            │           │
 │               │────────────────────────>│           │
 │               │ update     │            │           │
 │               │───────────>│            │           │
 │               │            │ add_entry  │           │
 │               │            │            │           │
 │ commit(msg)   │            │            │           │
 │──────────────>│            │            │           │
 │               │ build_tree │            │           │
 │               │───────────>│            │           │
 │               │ write_tree │            │           │
 │               │────────────────────────>│           │
 │               │<─ tree_oid ────────────│           │
 │               │ write_commit            │           │
 │               │────────────────────────>│           │
 │               │<─ commit_oid ──────────│           │
 │               │ update_head             │           │
 │               │─────────────────────────────────────>│
 │<─ oid ────────│            │            │           │
```

---

## 3. 設計原則・デザインパターン

### 3.1 設計原則

| 原則                     | 適用方法                                                         |
| ------------------------ | ---------------------------------------------------------------- |
| **単一責任原則 (SRP)**   | 各モジュールは1つの責務のみ担う（例: `hash` はハッシュ計算のみ） |
| **依存性逆転原則 (DIP)** | `infra` レイヤーをトレイトで抽象化し、テスト時にモック可能に     |
| **関心の分離**           | I/O（`infra`）とビジネスロジック（`domain`）を分離               |
| **早期リターン**         | エラー時は即座に `Result::Err` を返し、ネストを浅く保つ          |
| **防御的プログラミング** | 外部入力（ファイル、ユーザー入力）は常に検証                     |

### 3.2 デザインパターン

#### Builder パターン - オブジェクト生成

```rust
// Commitオブジェクトの構築（Phase 2）
let commit = CommitBuilder::new()
    .tree(tree_oid)
    .parent(parent_oid)
    .author(author)
    .committer(committer)
    .message("Initial commit")
    .build()?;
```

#### Iterator パターン - コミット履歴走査

```rust
// 遅延評価でコミットを走査
pub struct LogIterator<'a> {
    repo: &'a Repository,
    pending: VecDeque<Oid>,
    seen: HashSet<Oid>,
}

impl Iterator for LogIterator<'_> {
    type Item = Result<Commit>;
    
    fn next(&mut self) -> Option<Self::Item> {
        // 親コミットを遅延読み込み
    }
}
```

#### Newtype パターン - 型安全性

```rust
// SHA-1ハッシュを専用型で表現
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Oid([u8; 20]);

impl Oid {
    pub fn from_hex(s: &str) -> Result<Self>;
    pub fn to_hex(&self) -> String;
    pub fn short(&self) -> String;  // 先頭7文字
}
```

#### Repository パターン - データアクセス抽象化

```rust
// オブジェクトストアへのアクセスを抽象化
pub trait ObjectStore {
    fn read(&self, oid: &Oid) -> Result<Object>;
    fn write(&self, obj: &Object) -> Result<Oid>;  // Phase 2
    fn exists(&self, oid: &Oid) -> bool;
}

// 実装: LooseObjectStore (Phase 1), PackfileStore (Phase 3)
```

### 3.3 エラーハンドリング戦略

```rust
// 自前のエラー型（thiserror不使用）
#[derive(Debug)]
pub enum Error {
    // I/O
    Io(std::io::Error),
    
    // パース
    InvalidObject { oid: Oid, reason: String },
    InvalidIndex { version: u32 },
    InvalidRef { name: String },
    
    // 論理エラー
    ObjectNotFound(Oid),
    RefNotFound(String),
    NotARepository(PathBuf),
    
    // 圧縮
    DecompressionFailed,
}

impl std::fmt::Display for Error { /* ... */ }
impl std::error::Error for Error { /* ... */ }

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
```

---

## 4. 技術選定

### 4.1 依存crate

| crate       | バージョン | 用途     | 選定理由                                  |
| ----------- | ---------- | -------- | ----------------------------------------- |
| miniz_oxide | 0.8        | zlib解凍 | Pure Rust、追加依存なし、広く使われている |

### 4.2 自前実装

| 機能                  | 理由                                |
| --------------------- | ----------------------------------- |
| SHA-1                 | 約100行で実装可能、依存を増やさない |
| Gitオブジェクトパース | Git固有形式、既存crateなし          |
| Indexパース           | バイナリ形式、既存crateなし         |

### 4.3 将来の依存候補（Phase 2以降）

| crate   | 用途                         | 条件                       |
| ------- | ---------------------------- | -------------------------- |
| memmap2 | 大規模ファイルのメモリマップ | 性能要件次第               |
| rayon   | 並列処理                     | status()高速化が必要な場合 |

### 4.4 テスト戦略

| レベル         | 対象             | 手法                                 |
| -------------- | ---------------- | ------------------------------------ |
| ユニットテスト | 各モジュール     | `#[cfg(test)]`                       |
| 統合テスト     | Repository操作   | `tests/` ディレクトリ                |
| Fixture        | テストリポジトリ | `tests/fixtures/` に実際の.gitを配置 |

---

## 5. 主要な構造体定義（概要）

```rust
// === Public API ===

pub struct Repository {
    path: PathBuf,
    objects: LooseObjectStore,
    refs: RefStore,
}

// === Objects ===

pub enum Object {
    Blob(Blob),
    Tree(Tree),
    Commit(Commit),
    Tag(Tag),
}

pub struct Blob {
    pub content: Vec<u8>,
}

pub struct Tree {
    pub entries: Vec<TreeEntry>,
}

pub struct TreeEntry {
    pub mode: FileMode,
    pub name: String,
    pub oid: Oid,
}

pub struct Commit {
    pub tree: Oid,
    pub parents: Vec<Oid>,
    pub author: Signature,
    pub committer: Signature,
    pub message: String,
}

pub struct Signature {
    pub name: String,
    pub email: String,
    pub time: i64,
    pub offset: i32,
}

// === Status ===

pub struct StatusEntry {
    pub path: PathBuf,
    pub status: FileStatus,
}

pub enum FileStatus {
    Untracked,
    Modified,
    Staged,
    Deleted,
    Renamed { from: PathBuf },
}

// === Index ===

pub struct Index {
    pub version: u32,
    pub entries: Vec<IndexEntry>,
}

pub struct IndexEntry {
    pub oid: Oid,
    pub path: PathBuf,
    pub mode: FileMode,
    pub size: u32,
    pub mtime: u64,
    // ... その他メタデータ
}
```

---

## 6. Phase 1 MVP スコープ

MVP完了時に動作する機能：

```rust
// ✅ MVP スコープ
let repo = Repository::open(".")?;
let repo = Repository::discover("/some/path")?;

// コミット履歴
for commit in repo.log()?.take(10) {
    println!("{} {}", commit.id.short(), commit.summary());
}

// 特定コミット
let commit = repo.commit("abc1234")?;
println!("{:?}", commit.author);

// ステータス
for entry in repo.status()? {
    println!("{:?} {}", entry.status, entry.path);
}

// ブランチ一覧
for branch in repo.branches()? {
    println!("{}", branch.name);
}

// HEAD
let head = repo.head()?;
```

---

## 7. 参考実装

| 実装                                                   | 参考にする点                 |
| ------------------------------------------------------ | ---------------------------- |
| [gitoxide](https://github.com/Byron/gitoxide)          | オブジェクトパース、全体構造 |
| [git (C実装)](https://github.com/git/git)              | 正式な仕様・挙動の確認       |
| [dulwich (Python)](https://github.com/dulwich/dulwich) | シンプルな実装例             |
