# Issue #034: Repository::init()メソッドを追加

## 基本情報

| 項目 | 内容 |
|------|------|
| Phase | 2.5 |
| 優先度 | 中 |
| 難易度 | 低 |
| 推定時間 | 2h |
| 依存 | #016 |
| GitHub Issue | https://github.com/siska-tech/zerogit/issues/3 |
| ステータス | 完了 |

## 説明

新規Gitリポジトリを初期化する`Repository::init()`メソッドを追加する。

## 背景

以前、`Repository`は既存のリポジトリを開く方法のみを提供していた：

- `Repository::open()` - 指定パスのリポジトリを開く
- `Repository::discover()` - 親ディレクトリを辿ってリポジトリを探す

新規リポジトリを作成する機能がなかったため、git CLIにフォールバックする必要があった。

## タスク

- [x] `Repository::init()`を実装
- [x] 必要なディレクトリ構造を作成
  - `.git/objects`
  - `.git/refs/heads`
  - `.git/refs/tags`
- [x] `HEAD`ファイルを作成（`ref: refs/heads/main`）
- [x] 最小限の`config`ファイルを作成
- [x] テストを作成

## 想定API

```rust
use zerogit::{Repository, Result};

fn main() -> Result<()> {
    // 新しいGitリポジトリを作成
    let repo = Repository::init("./my-project")?;

    println!("Initialized empty Git repository");
    Ok(())
}
```

## 実装

```rust
impl Repository {
    /// Initializes a new Git repository at the given path.
    pub fn init<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let git_dir = path.join(".git");

        // Create directory structure
        fs::create_dir_all(&git_dir)?;
        fs::create_dir_all(git_dir.join("objects"))?;
        fs::create_dir_all(git_dir.join("refs/heads"))?;
        fs::create_dir_all(git_dir.join("refs/tags"))?;

        // Create HEAD file
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n")?;

        // Create config file (minimal)
        fs::write(git_dir.join("config"), "[core]\n\trepositoryformatversion = 0\n")?;

        Self::open(path)
    }
}
```

## 受け入れ条件

- [x] `Repository::init()`で新規リポジトリを作成できる
- [x] 必要なディレクトリ構造が作成される
- [x] HEADファイルが`refs/heads/main`を指す
- [x] 作成後に`Repository::open()`で開ける

## 将来の拡張

- `init_bare()` - bareリポジトリの初期化
- デフォルトブランチ名の設定オプション（`main` vs `master`）
- 初期設定のカスタマイズ

## 関連Issue

- #016: Repository基本実装（基盤）
- #035: Git Config読み取り（設定ファイルの読み取り）
