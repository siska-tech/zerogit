# zerogit

Pure Rust製の軽量Gitクライアントライブラリ。最小限の依存でGitリポジトリの読み書きを実現します。

[![Crates.io](https://img.shields.io/crates/v/zerogit.svg)](https://crates.io/crates/zerogit)
[![Documentation](https://docs.rs/zerogit/badge.svg)](https://docs.rs/zerogit)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](#license)

## 特徴

- **Pure Rust**: Cバインディングなし、クロスコンパイルが容易
- **最小依存**: `miniz_oxide`（zlib解凍）のみに依存
- **軽量**: 必要な機能だけを実装したシンプルな設計
- **学習向け**: Git内部構造の理解に役立つクリーンな実装

## インストール

`Cargo.toml` に以下を追加：

```toml
[dependencies]
zerogit = "0.2"
```

### 必要環境

- Rust 1.70.0 以上
- Linux / macOS / Windows

## クイックスタート

### リポジトリを開いてログを表示

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

### ステータスを確認

```rust
use zerogit::{Repository, FileStatus, Result};

fn main() -> Result<()> {
    let repo = Repository::open(".")?;
    
    for entry in repo.status()? {
        let marker = match entry.status() {
            FileStatus::Untracked => "??",
            FileStatus::Modified => " M",
            FileStatus::Added => "A ",
            FileStatus::Deleted => " D",
            _ => "  ",
        };
        println!("{} {}", marker, entry.path().display());
    }
    
    Ok(())
}
```

### 特定コミットの詳細を取得

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;
    
    // 短縮形式でもOK
    let commit = repo.commit("abc1234")?;
    
    println!("Commit:  {}", commit.oid());
    println!("Author:  {} <{}>", commit.author().name(), commit.author().email());
    println!("Message: {}", commit.summary());
    
    Ok(())
}
```

### ブランチ一覧

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

## API概要

### 主要な型

| 型           | 説明                                       |
| ------------ | ------------------------------------------ |
| `Repository` | リポジトリ操作のエントリーポイント         |
| `Commit`     | コミット情報（author, message, parents等） |
| `Tree`       | ディレクトリ構造                           |
| `Blob`       | ファイル内容                               |
| `Oid`        | オブジェクトID（SHA-1ハッシュ）            |
| `Branch`     | ブランチ情報                               |
| `Head`       | HEAD参照（ブランチまたはdetached）         |

### Repository メソッド

```rust
// リポジトリを開く
Repository::open(path)?;      // 指定パス
Repository::discover(path)?;  // 親ディレクトリを探索

// 読み取り操作
repo.head()?;                 // HEAD取得
repo.branches()?;             // ブランチ一覧
repo.log()?;                  // コミット履歴（Iterator）
repo.status()?;               // ワーキングツリー状態
repo.commit("sha")?;          // コミット取得
repo.tree("sha")?;            // ツリー取得
repo.blob("sha")?;            // Blob取得
repo.index()?;                // インデックス取得

// 書き込み操作
repo.add(path)?;              // ファイルをステージ
repo.add_all()?;              // 全変更をステージ
repo.reset(path)?;            // ステージを解除
repo.create_commit(msg, author, email)?;  // コミット作成
repo.create_branch(name, target)?;        // ブランチ作成
repo.delete_branch(name)?;                // ブランチ削除
repo.checkout(target)?;                   // ブランチ切り替え
```

詳細は [APIドキュメント](https://docs.rs/zerogit) を参照してください。

## 使用例

### ファイル内容の取得

```rust
let repo = Repository::discover(".")?;
let head = repo.head()?;
let commit = repo.commit(&head.oid().to_hex())?;
let tree = repo.tree(&commit.tree().to_hex())?;

if let Some(entry) = tree.get("README.md") {
    let blob = repo.blob(&entry.oid().to_hex())?;
    println!("{}", blob.content_str()?);
}
```

### コミット間の差分ファイル一覧

```rust
let repo = Repository::discover(".")?;
let commits: Vec<_> = repo.log()?.take(2).collect::<Result<_, _>>()?;

if commits.len() == 2 {
    let tree1 = repo.tree(&commits[0].tree().to_hex())?;
    let tree2 = repo.tree(&commits[1].tree().to_hex())?;

    // ツリーを比較して差分を検出
    // ...
}
```

### ファイルをステージしてコミット

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;

    // ファイルをステージ
    repo.add("src/main.rs")?;

    // または全変更をステージ
    repo.add_all()?;

    // コミット作成
    let oid = repo.create_commit(
        "Add new feature",
        "Your Name",
        "your@email.com"
    )?;

    println!("Created commit: {}", oid.short());
    Ok(())
}
```

### ブランチ操作

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;

    // 新しいブランチを作成
    repo.create_branch("feature/new-feature", None)?;

    // ブランチに切り替え
    repo.checkout("feature/new-feature")?;

    // 作業後、mainに戻る
    repo.checkout("main")?;

    // ブランチを削除
    repo.delete_branch("feature/new-feature")?;

    Ok(())
}
```

## ロードマップ

### Phase 1: 読み取り操作（MVP）✅

ローカルリポジトリの読み取り機能を提供します。

- [x] オブジェクト読み取り（blob/tree/commit）
- [x] 参照解決（HEAD/branches/tags）
- [x] コミット履歴イテレータ
- [x] ワーキングツリーステータス
- [x] インデックス読み取り

### Phase 2: 書き込み操作

ローカルリポジトリへの書き込み機能を提供します。

- [x] `add` / `reset` - ステージング操作
- [x] `commit` - コミット作成
- [x] `branch` - ブランチ作成・削除
- [x] `checkout` - ブランチ切り替え

### Phase 3: 差分・マージ・Packfile

差分計算とPackfile対応を提供します。

| 機能                 | 説明                                  | 難易度 |
| -------------------- | ------------------------------------- | ------ |
| Tree diff            | 2つのTree間の差分計算                 | 中     |
| Blob diff            | ファイル内容の行単位差分（Myers算法） | 高     |
| Packfile読み取り     | `.git/objects/pack/*.pack` の読み取り | 高     |
| Packfileインデックス | `.idx` ファイルによる高速検索         | 中     |
| Delta復元            | ofs_delta / ref_delta の展開          | 高     |
| 3-way merge          | 共通祖先ベースのマージ                | 高     |

**想定API:**
```rust
// 差分取得
let diff = repo.diff_trees(&tree1, &tree2)?;
for delta in diff.deltas() {
    println!("{:?} {}", delta.status(), delta.path());
}

// Packfile対応（内部的に自動処理）
let obj = repo.object("abc123")?;  // looseまたはpackから透過的に取得
```

### Phase 4: リモート操作（別crate: `zerogit-remote`）

ネットワーク操作は依存関係が増えるため、別crateとして提供予定です。

#### なぜ別crateなのか？

| 観点         | zerogit (コア)     | zerogit-remote               |
| ------------ | ------------------ | ---------------------------- |
| 依存         | `miniz_oxide` のみ | `rustls`, `russh`, `ureq` 等 |
| ビルド時間   | 高速               | TLS/SSH依存で増加            |
| WASM対応     | ○                  | △（制限あり）                |
| 組み込み用途 | ○                  | △                            |

#### サポート予定プロトコル

| プロトコル | URL形式                  | 認証方式             | 優先度 |
| ---------- | ------------------------ | -------------------- | ------ |
| HTTPS      | `https://github.com/...` | Basic / Bearer Token | 高     |
| SSH        | `git@github.com:...`     | SSH鍵                | 中     |
| Git        | `git://...`              | なし（読み取り専用） | 低     |

#### 想定API

```rust
use zerogit::Repository;
use zerogit_remote::{Remote, Credentials};

// クローン
let repo = Remote::clone(
    "https://github.com/user/repo.git",
    "./local-repo",
    Credentials::token("ghp_xxxx"),
)?;

// フェッチ
let remote = repo.remote("origin")?;
remote.fetch(&Credentials::ssh_key("~/.ssh/id_ed25519"))?;

// プッシュ
remote.push("main", &Credentials::token("ghp_xxxx"))?;
```

#### 技術的な実装要素

```
Smart HTTP Protocol:
┌─────────┐                              ┌─────────┐
│ Client  │  GET /info/refs              │ Server  │
│         │ ───────────────────────────> │         │
│         │  200 OK (refs + capabilities)│         │
│         │ <─────────────────────────── │         │
│         │                              │         │
│         │  POST /git-upload-pack       │         │
│         │  (want/have negotiation)     │         │
│         │ ───────────────────────────> │         │
│         │  200 OK (packfile)           │         │
│         │ <─────────────────────────── │         │
└─────────┘                              └─────────┘
```

#### 代替アプローチ

リモート操作が必要だが `zerogit-remote` を待てない場合、システムのgitコマンドと連携できます：

```rust
use std::process::Command;

fn fetch_with_git(repo_path: &str, remote: &str) -> std::io::Result {
    Command::new("git")
        .args(["-C", repo_path, "fetch", remote])
        .status()?;
    Ok(())
}
```

### 将来の検討事項

- **Worktree対応**: 複数のワーキングツリー
- **Submodule対応**: サブモジュールの読み取り
- **Sparse checkout**: 部分的なチェックアウト
- **Shallow clone**: 履歴を限定したクローン

## 貢献

コントリビューションを歓迎します！

### 開発環境のセットアップ

```bash
git clone https://github.com/siska-tech/zerogit
cd zerogit

# テスト用フィクスチャの準備
cd tests/fixtures
bash create_fixtures.sh
cd ../..

# テスト実行
cargo test

# フォーマットとLint
cargo fmt
cargo clippy
```

### プルリクエスト

1. Issueを作成して変更内容を議論
2. フォークしてfeatureブランチを作成
3. 変更を実装（テスト必須）
4. `cargo fmt` と `cargo clippy` を実行
5. プルリクエストを送信

### コーディング規約

- `cargo fmt` でフォーマット
- `cargo clippy` の警告をゼロに
- 公開APIには必ずドキュメントコメント
- 新機能にはテストを追加

## 設計ドキュメント

詳細な設計については以下を参照：

- [要件定義書](docs/zerogit-requirements.md)
- [アーキテクチャ設計書](docs/zerogit-architecture.md)
- [インターフェース設計書](docs/zerogit-interface.md)
- [詳細設計書](docs/zerogit-detailed-design.md)
- [テスト仕様書](docs/zerogit-test-spec.md)

## 関連プロジェクト

- [gitoxide](https://github.com/Byron/gitoxide) - フル機能のPure Rust Git実装
- [git2-rs](https://github.com/rust-lang/git2-rs) - libgit2のRustバインディング

zerogitは学習目的と軽量な用途に特化しています。フル機能が必要な場合は上記のプロジェクトを検討してください。

## ライセンス

本プロジェクトはデュアルライセンスです：

- [MIT License](LICENSE-MIT)
- [Apache License 2.0](LICENSE-APACHE)

お好きな方を選択してください。

## 謝辞

- [Git](https://git-scm.com/) - オリジナル実装とドキュメント
- [Pro Git Book](https://git-scm.com/book) - Git内部構造の解説
- [gitoxide](https://github.com/Byron/gitoxide) - Pure Rust実装の参考
