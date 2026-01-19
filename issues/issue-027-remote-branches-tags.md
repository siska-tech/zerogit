# Issue #027: リモートブランチ・タグ一覧対応

## 基本情報

| 項目                   | 内容                       |
| ---------------------- | -------------------------- |
| Phase                  | 1.5                        |
| 優先度                 | 高                         |
| 難易度                 | 低                         |
| 推定時間               | 3h                         |
| 依存                   | #013, #014                 |
| 解消するフォールバック | `git branch -r`, `git tag` |

## 説明

refs/remotes/* と refs/tags/* の読み取りに対応し、branches機能を完全にする。
現在の実装ではローカルブランチ（refs/heads/*）のみ対応しているため、リモートブランチとタグの一覧取得にgit CLIへのフォールバックが必要になっている。

## 背景

```rust
// 現在対応済み
repo.branches()?;           // refs/heads/* のみ

// 未対応（git CLIフォールバックが必要）
repo.remote_branches()?;    // refs/remotes/*
repo.tags()?;               // refs/tags/*
```

## タスク

- [x] `src/refs/remote_branch.rs` を作成
- [x] `RemoteBranch` 型を定義（remote名 + branch名）
- [x] `Repository::remote_branches()` を実装
- [x] `src/refs/tag.rs` を作成
- [x] `Tag` 型を定義（軽量タグ / 注釈付きタグ対応）
- [x] `Repository::tags()` を実装
- [x] `RefStore` にremotes/tags走査を追加
- [x] 注釈付きタグ（tag object）のパースを実装
- [x] 統合テストを作成

## 想定API

```rust
// リモートブランチ一覧
for rb in repo.remote_branches()? {
    println!("{}/{}", rb.remote(), rb.name());
    // 出力例: origin/main, origin/develop, upstream/main
}

// タグ一覧
for tag in repo.tags()? {
    println!("{} -> {}", tag.name(), tag.target().short());
    
    // 注釈付きタグの場合はメッセージも取得可能
    if let Some(message) = tag.message() {
        println!("  {}", message);
    }
}

// 特定のリモートのブランチのみ
for rb in repo.remote_branches()?.filter(|b| b.remote() == "origin") {
    println!("{}", rb.name());
}
```

## データ構造

```rust
/// リモートブランチ
#[derive(Debug, Clone)]
pub struct RemoteBranch {
    /// リモート名（例: "origin"）
    remote: String,
    /// ブランチ名（例: "main"）
    name: String,
    /// 指すコミットのOID
    oid: Oid,
}

impl RemoteBranch {
    pub fn remote(&self) -> &str { &self.remote }
    pub fn name(&self) -> &str { &self.name }
    pub fn full_name(&self) -> String { format!("{}/{}", self.remote, self.name) }
    pub fn oid(&self) -> &Oid { &self.oid }
}

/// タグ
#[derive(Debug, Clone)]
pub struct Tag {
    /// タグ名
    name: String,
    /// タグが指すオブジェクト（コミットまたはタグオブジェクト）
    target: Oid,
    /// 注釈付きタグの場合のメッセージ
    message: Option<String>,
    /// 注釈付きタグの場合の作成者
    tagger: Option<Signature>,
}

impl Tag {
    pub fn name(&self) -> &str { &self.name }
    pub fn target(&self) -> &Oid { &self.target }
    pub fn message(&self) -> Option<&str> { self.message.as_deref() }
    pub fn tagger(&self) -> Option<&Signature> { self.tagger.as_ref() }
    pub fn is_annotated(&self) -> bool { self.message.is_some() }
}
```

## 実装詳細

### refs/remotes の構造

```
.git/refs/remotes/
├── origin/
│   ├── main
│   ├── develop
│   └── feature/xyz
└── upstream/
    └── main
```

### 注釈付きタグオブジェクトの形式

```
tag v1.0.0
object abc123...  (指すコミットのSHA)
type commit
tagger John Doe <john@example.com> 1700000000 +0900

Release version 1.0.0

詳細なリリースノート...
```

## テストケース

| ID     | テスト項目           | 条件                         | 期待結果                 |
| ------ | -------------------- | ---------------------------- | ------------------------ |
| RB-001 | リモートブランチ一覧 | refs/remotes/origin/* が存在 | 全リモートブランチを取得 |
| RB-002 | 複数リモート         | origin, upstream が存在      | 両方のブランチを取得     |
| RB-003 | ネストしたブランチ   | feature/xyz 形式             | 正しくパース             |
| RB-004 | リモートなし         | refs/remotes が空            | 空のVec                  |
| T-001  | 軽量タグ一覧         | refs/tags/* が存在           | 全タグを取得             |
| T-002  | 注釈付きタグ         | tag objectが存在             | message, taggerを取得    |
| T-003  | 混在                 | 軽量・注釈付き両方           | 両方を正しく取得         |

## テストフィクスチャ追加

```bash
# tests/fixtures/create_fixtures.sh に追加

# remotes: リモートブランチのあるリポジトリ
rm -rf remotes
mkdir -p remotes && cd remotes
git init
git config user.email "test@example.com"
git config user.name "Test User"
echo "main" > file.txt
git add file.txt
git commit -m "Initial commit"
# 疑似的にリモートブランチを作成
mkdir -p .git/refs/remotes/origin
cp .git/refs/heads/main .git/refs/remotes/origin/main
echo "$(git rev-parse HEAD)" > .git/refs/remotes/origin/develop
cd ..

# tags: タグのあるリポジトリ
rm -rf tags
mkdir -p tags && cd tags
git init
git config user.email "test@example.com"
git config user.name "Test User"
echo "v1" > file.txt
git add file.txt
git commit -m "Version 1"
git tag v1.0.0  # 軽量タグ
git tag -a v1.0.1 -m "Annotated tag"  # 注釈付きタグ
cd ..
```

## 受け入れ条件

- [x] `git branch -r` 相当の出力が得られる
- [x] `git tag` 相当の出力が得られる
- [x] 注釈付きタグのメッセージを取得できる
- [x] ネストしたリモートブランチ名（feature/xyz）を正しく処理
- [x] テスト RB-001〜RB-004, T-001〜T-003 がパス

## 公開API変更

```rust
// src/lib.rs に追加
pub use refs::{RemoteBranch, Tag};

// Repository に追加
impl Repository {
    pub fn remote_branches(&self) -> Result<Vec<RemoteBranch>>;
    pub fn tags(&self) -> Result<Vec<Tag>>;
}
```

## 関連Issue

- #013: 参照解決実装（基盤）
- #014: Head/Branch型実装（基盤）
