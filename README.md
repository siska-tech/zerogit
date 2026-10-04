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
zerogit = "0.4"
```

### 必要環境

- Rust 1.70.0 以上
- Linux / macOS / Windows

## クイックスタート

### 新規リポジトリを初期化

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    // 新しいGitリポジトリを作成
    let repo = Repository::init("./my-project")?;

    println!("Initialized empty Git repository");
    Ok(())
}
```

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

    // ローカルブランチ
    for branch in repo.branches()? {
        let marker = if head.branch().map(|b| b.name()) == Some(branch.name()) {
            "* "
        } else {
            "  "
        };
        println!("{}{}", marker, branch.name());
    }

    // リモートブランチ
    for rb in repo.remote_branches()? {
        println!("  remotes/{}/{}", rb.remote(), rb.name());
    }

    Ok(())
}
```

### タグ一覧

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;

    for tag in repo.tags()? {
        println!("{} -> {}", tag.name(), tag.target().short());

        // 注釈付きタグの場合はメッセージも取得可能
        if let Some(message) = tag.message() {
            println!("  {}", message);
        }
    }

    Ok(())
}
```

## API概要

### 主要な型

| 型             | 説明                                       |
| -------------- | ------------------------------------------ |
| `Repository`   | リポジトリ操作のエントリーポイント         |
| `Commit`       | コミット情報（author, message, parents等） |
| `Tree`         | ディレクトリ構造                           |
| `Blob`         | ファイル内容                               |
| `Oid`          | オブジェクトID（SHA-1ハッシュ）            |
| `Branch`       | ブランチ情報                               |
| `RemoteBranch` | リモートブランチ情報                       |
| `Tag`          | タグ情報（軽量/注釈付き）                  |
| `Head`         | HEAD参照（ブランチまたはdetached）         |
| `TreeDiff`     | Tree間の差分                               |
| `DiffDelta`    | 差分の各エントリ                           |
| `LogOptions`   | ログ取得オプション                         |
| `BlobDiff`     | 2つのBlob間の行差分（Text/NonText/Skipped） |
| `DiffHunk`     | 行差分のhunk（旧新の開始行・行数）         |
| `DiffLine`     | hunk内の1行（種別・旧新行番号・内容）      |
| `DiffOptions`  | 文脈行数・入力サイズ・計算量の上限         |

### Repository メソッド

```rust
// リポジトリを開く・作成する
Repository::init(path)?;      // 新規リポジトリを初期化
Repository::open(path)?;      // 指定パス
Repository::discover(path)?;  // 親ディレクトリを探索

// 読み取り操作
repo.head()?;                 // HEAD取得
repo.branches()?;             // ローカルブランチ一覧
repo.remote_branches()?;      // リモートブランチ一覧
repo.tags()?;                 // タグ一覧
repo.log()?;                  // コミット履歴（Iterator）
repo.reflog("HEAD")?;         // reflog（新しい順）
repo.log_with_options(opts)?; // フィルタリング付きログ
repo.status()?;               // ワーキングツリー状態（パスごとに1つの状態）
repo.detailed_status()?;      // index側・作業ツリー側を別々に（porcelain v2相当）
repo.commit("sha")?;          // コミット取得
repo.tree("sha")?;            // ツリー取得
repo.blob("sha")?;            // Blob取得
repo.index()?;                // インデックス取得
repo.is_ignored(path)?;       // .gitignore等で無視されるか
repo.ignored_files()?;        // 無視された未追跡ファイル一覧

// 差分操作
repo.diff_trees(old, new)?;       // Tree間の差分
repo.commit_diff(&commit)?;       // コミットの変更ファイル一覧
repo.diff_index_to_workdir()?;    // git diff 相当
repo.diff_head_to_index()?;       // git diff --staged 相当
repo.diff_head_to_workdir()?;     // git diff HEAD 相当
repo.diff_blobs(old, new, &opts)?; // Blob間の行差分（旧新行番号付き）
repo.resolve_short_oid("abc1234")?; // 短縮OIDの解決（loose・pack横断）

// 書き込み操作
repo.add(path)?;              // ファイルをステージ（無視対象の未追跡ファイルは拒否）
repo.add_force(path)?;        // 無視対象でもステージ（git add -f）
repo.add_all()?;              // 全変更をステージ
repo.reset(path)?;            // ステージを解除
repo.create_commit(msg, author, email)?;  // コミット作成
repo.create_branch(name, target)?;        // ブランチ作成
repo.delete_branch(name)?;                // ブランチ削除
repo.checkout(target)?;                   // ブランチ切り替え
repo.create_tag(name, target)?;           // 軽量タグ作成
repo.create_annotated_tag(name, target, msg, tagger, email)?; // 注釈付きタグ作成
repo.delete_tag(name)?;                   // タグ削除
repo.merge(target, name, email, &MergeOptions::new())?; // マージ（ff・3-way）
repo.abort_merge()?;                      // マージの中止
repo.merge_base(&a, &b)?;                 // 共通祖先
repo.stash_save(name, email, &StashOptions::new())?; // 変更の退避
repo.stash_list()?;                       // stash一覧
repo.stash_pop(0, false)?;                // 復元して削除（apply/dropも）
repo.rebase("main", None, name, email)?;  // rebase（continue/skip/abortも）
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

### コミットの変更ファイル一覧

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;

    // 最新コミットの変更ファイルを表示
    for commit in repo.log()?.take(5) {
        let commit = commit?;
        let diff = repo.commit_diff(&commit)?;

        println!("{} {}", commit.oid().short(), commit.summary());
        for delta in diff.deltas() {
            println!("  {} {}", delta.status_char(), delta.path().display());
        }
    }

    Ok(())
}
```

### 行単位差分（旧新行番号付き）

変更一覧はTreeの比較だけで取得し、Blobは詳細表示するファイルについてのみ読み込みます。比較開始時にOIDを固定しておけば、途中でブランチが動いたり`git gc`が走ったりしても同じ比較を続けられます。

```rust
use zerogit::{BlobDiffContent, DiffOptions, FileMode, Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;
    let commit_oid = *repo.head()?.oid(); // 比較対象をOIDで固定
    let commit = repo.commit(&commit_oid.to_hex())?;
    let changes = repo.commit_diff(&commit)?; // 第一親との比較（初回コミットは空Treeとの比較）

    for delta in changes.deltas() {
        if delta.new_mode() == Some(FileMode::Submodule) {
            continue; // gitlinkはこのリポジトリのBlobではない
        }
        let diff = repo.diff_blobs(delta.old_oid(), delta.new_oid(), &DiffOptions::new())?;
        match diff.content() {
            BlobDiffContent::Text(hunks) => {
                for hunk in hunks {
                    println!("{}", hunk.header());
                    for line in hunk.lines() {
                        println!("{:?} {:?} {:?} {}", line.kind(), line.old_lineno(), line.new_lineno(), line.text());
                    }
                }
            }
            BlobDiffContent::NonText(reason) => println!("テキストではありません: {:?}", reason),
            BlobDiffContent::Skipped(reason) => println!("差分を省略: {}", reason),
        }
    }
    Ok(())
}
```

マージコミットの別の親や任意の2コミットを比較する場合は、`diff_trees(Some(&base_tree), &tree)`を使います。一連の流れは[`examples/document_diff.rs`](examples/document_diff.rs)にまとめています（`cargo run --example document_diff -- <repo> [--commit <oid>] [--parent <n>] [<path>...]`）。

### ログフィルタリング

```rust
use zerogit::{Repository, LogOptions, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;

    // 特定ファイルの変更履歴を取得
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

### ワーキングツリーの差分

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    let repo = Repository::discover(".")?;

    // git diff 相当（未ステージの変更）
    let unstaged = repo.diff_index_to_workdir()?;
    println!("Unstaged changes:");
    for delta in unstaged.deltas() {
        println!("  {} {}", delta.status_char(), delta.path().display());
    }

    // git diff --staged 相当（ステージ済みの変更）
    let staged = repo.diff_head_to_index()?;
    println!("Staged changes:");
    for delta in staged.deltas() {
        println!("  {} {}", delta.status_char(), delta.path().display());
    }

    Ok(())
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

### Phase 2: 書き込み操作 ✅

ローカルリポジトリへの書き込み機能を提供します。

- [x] `add` / `reset` - ステージング操作
- [x] `commit` - コミット作成
- [x] `branch` - ブランチ作成・削除
- [x] `checkout` - ブランチ切り替え

### Phase 2.5: 参照拡張・ログフィルタリング・差分機能 ✅

リモートブランチ、タグ、ログフィルタリング、Tree diff機能を提供します。

- [x] リモートブランチ一覧（`remote_branches()`）
- [x] タグ一覧（`tags()`）- 軽量タグ・注釈付きタグ両対応
- [x] ログフィルタリング（`log_with_options()`）- パス、件数、日付、作者
- [x] Tree diff（`diff_trees()`）- リネーム検出対応
- [x] コミット変更一覧（`commit_diff()`）
- [x] ワーキングツリー差分（`diff_index_to_workdir()`, `diff_head_to_index()`）

### Phase 3: Packfile・行単位差分・マージ ✅

- [x] Packfile読み取り - pack v2/v3、idx v2（64bit offset含む）、OFS_DELTA/REF_DELTA、多段delta。looseと複数packを透過的に扱い、`git repack`/`git gc`後も開いたままの`Repository`で読み続けられる
- [x] `packed-refs` - loose参照を優先し、HEAD・ブランチ・リモートブランチ・タグを解決・列挙。packed参照の削除は参照の復活を防ぐため`Error::PackedRefDeletionUnsupported`
- [x] 行単位差分（`diff_blobs()` / `BlobDiff::compute()`）- 最小編集のMyers法、旧新行番号、文脈行数指定、LF/CRLF・末尾改行の保持
- [x] 完全一致リネームの旧新mode保持と決定的な対応付け
- [x] 類似度によるリネーム検出（任意）- `diff_trees_with_options()`/`commit_diff_with_options()`に`RenameOptions::new().detection(RenameDetection::Similar)`を渡す。既定は完全一致のみ
- [x] 3-way merge（`merge()`）- fast-forward、共通祖先（複数の場合は仮想祖先）ベースのマージ、Gitと同じ形のコンフリクト記録。内容のマージは`git merge`と同じhistogram diff
- [x] stash（`stash_save()`など）・rebase（`rebase()`など）- Gitと同じ形式で状態を保存し、Gitと相互に扱える

#### 対応形式と制限

| 項目 | 対応 |
| --- | --- |
| オブジェクト形式 | SHA-1のみ。SHA-256（`extensions.objectFormat`）は`Error::UnsupportedRepositoryFormat` |
| オブジェクト格納 | loose、pack v2/v3（idx v2）。idx v1・multi-pack-index・commit-graph・bitmapは使用しない（packの`.idx`を直接読む） |
| 参照 | loose refs、`packed-refs`。reftableは`Error::UnsupportedRepositoryFormat` |
| index | v2/v3/v4（読んだバージョンで書き戻す）。split index・sparse indexは`Error::UnsupportedIndex`。sparse checkout（skip-worktree）中の全体reset・checkoutは未対応 |
| 未対応 | merge時のリネーム検出、対話的rebase、worktree、shallow/partial clone、alternates |
| 差分の結果 | `Text`（完全な行差分）、`NonText`（NULを含む・不正UTF-8。暗黙の置換はしない）、`Skipped`（サイズ・計算量の上限超過。部分結果は返さない） |
| リネーム検出 | 既定は完全一致のみ。類似度検出（任意）は通常・実行ファイルのテキストが対象で、類似度は「共通する行のバイト数 ÷ 大きい方のサイズ」。既定しきい値50%、候補ペア10万組、1ファイル1 MiBまで。上限に達した分は追加・削除のまま残り、`TreeDiff::rename_limits()`で識別できる |
| パス | `DiffDelta::path()`はプラットフォームの`PathBuf`（Windowsでは`\`区切り）。比較は`Path`同士で行う |

破損したpack・idx・deltaは`Error::InvalidPack`/`Error::InvalidPackIndex`、上限超過は`Error::PackLimitExceeded`となり、空の履歴や差分として扱われることはありません。

#### 処理上限の既定値

| 設定 | 既定値 | 根拠 |
| --- | --- | --- |
| `DiffOptions::context_lines` | 3 | `git diff`と同じ |
| `DiffOptions::max_input_size` | 8 MiB/片側 | 20,000行（1.3 MB）の全置換でも約16 ms。8 MiBは一般的な文書を十分に上回る |
| `DiffOptions::max_cost` | 5,000万ステップ | 最悪ケース（同じ行が多数繰り返される5,000行文書の全置換）が約1,700万ステップ・約90 ms。2万行の同ケースは約2.7億ステップ・約3.4秒のため省略される |
| `PackLimits::max_object_size` | 1 GiB | 宣言サイズによる過大な確保を防止 |
| `PackLimits::max_delta_depth` | 10,000 | Gitの`--depth`上限4,095を上回る |
| `PackLimits::delta_cache_size` | 32 MiB/pack | 多段deltaの再展開を回避 |

測定は`cargo run --release --example measure_document_diff`で再現できます（Windows 11、release build。1,000コミット・50文書をpack化したリポジトリで、全履歴の走査50〜220 ms、全コミットの変更一覧160〜670 ms、全変更ファイルの行差分は1件あたり0.6〜2.5 ms、ピークメモリ約46 MiB）。

### Phase 4: リモート操作（別crate: `zerogit-remote`）✅

ネットワーク操作は依存関係が増えるため、別crate [`zerogit-remote`](zerogit-remote/) として提供します。コアの`zerogit`の依存は`miniz_oxide`のみのままです。

| 観点         | zerogit (コア)     | zerogit-remote                     |
| ------------ | ------------------ | ---------------------------------- |
| 依存         | `miniz_oxide` のみ | `zerogit`、`ureq`（`rustls`、`https` feature） |
| ビルド時間   | 高速               | TLS依存で増加                      |
| WASM対応     | ○                  | △（制限あり）                      |

| プロトコル | URL形式 | 認証方式 | 状態 |
| --- | --- | --- | --- |
| ローカル | パス、`file://` | なし | ✅（Gitを起動せず直接読み書き） |
| HTTPS / HTTP | `https://...` | Basic（URLまたは`HttpAuth`）/ Bearer Token | ✅ Smart HTTP、protocol v2 |
| SSH | `git@host:path`、`ssh://...` | SSH鍵（システムの`ssh`クライアントとagent） | ✅ |
| Git | `git://...` | なし | 未対応 |

```rust
use std::path::Path;
use zerogit_remote::{clone, fetch, push, CloneOptions, PushOptions};

let repo = clone("https://github.com/user/repo.git", Path::new("./local-repo"), &CloneOptions::new())?;
fetch(&repo, "origin")?;
push(&repo, "origin", &["main"], &PushOptions::new())?;
```

コア側には、受け取ったpackの検証・保存（`store_pack()`、thin packの補完）、送るpackの作成（`pack_objects()`）、リモート設定とrefspec（`remotes()`、`add_remote()`、`Refspec`）、upstream（`set_branch_upstream()`）、参照の更新（`update_reference()`）を追加しました。shallow・partial cloneは対象外です。

### 将来の検討事項

- **Worktree対応**: 複数のワーキングツリー
- **Submodule対応**: サブモジュールの読み取り
- **Sparse checkout**: 部分的なチェックアウト
- **Shallow / partial clone**: 履歴・オブジェクトを限定したクローン

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
