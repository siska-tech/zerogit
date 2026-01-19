# Issue #036: Git Config読み取りAPIを追加

## 基本情報

| 項目 | 内容 |
|------|------|
| Phase | 2.5 |
| 優先度 | 中 |
| 難易度 | 中 |
| 推定時間 | 3h |
| 依存 | #016 |
| GitHub Issue | https://github.com/siska-tech/zerogit/issues/5 |
| ステータス | 完了 |

## 説明

Git設定ファイル（`.git/config`）を読み取るAPIを追加する。

## 背景

以前、zerogitを使用するプロジェクトでは`user.name`や`user.email`などのGit設定を取得するためにgit CLIにフォールバックしていた：

```rust
// 以前: git CLIに依存
fn get_git_config() -> (String, String) {
    let name = Command::new("git")
        .args(["config", "user.name"])
        .output();
    // ...
}
```

zerogitにConfig読み取り機能を追加することで、完全にpure Rustで完結できるようになった。

## タスク

- [x] `src/config.rs`を作成
- [x] `Config`構造体を定義
- [x] INI形式のパーサーを実装
- [x] `Repository::config()`を実装
- [x] `Config::get()`を実装
- [x] テストを作成

## 想定API

```rust
let repo = Repository::open(".")?;
let config = repo.config()?;

let name = config.get("user.name").unwrap_or("Unknown");
let email = config.get("user.email").unwrap_or("unknown@example.com");

println!("Author: {} <{}>", name, email);
```

## データ構造

```rust
/// Git configuration.
#[derive(Debug, Clone)]
pub struct Config {
    entries: HashMap<String, String>,
}

impl Config {
    /// Gets a configuration value by key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(|s| s.as_str())
    }

    /// Gets a configuration value with a default.
    pub fn get_or(&self, key: &str, default: &str) -> &str {
        self.get(key).unwrap_or(default)
    }
}
```

## Git configファイルの形式

```ini
[core]
    repositoryformatversion = 0
    filemode = true
    bare = false
[user]
    name = Your Name
    email = your@email.com
[remote "origin"]
    url = https://github.com/user/repo.git
    fetch = +refs/heads/*:refs/remotes/origin/*
```

- セクション名にサブセクション（`"origin"`）を含む場合がある
- 値にはスペースや特殊文字を含む場合がある
- コメントは`#`または`;`で始まる

## 受け入れ条件

- [x] `Repository::config()`で設定を取得できる
- [x] `Config::get("user.name")`で値を取得できる
- [x] セクション.キー形式（`user.name`）で正しく取得できる
- [x] サブセクション（`remote.origin.url`）も取得できる

## 将来の拡張（Phase 2, 3）

- グローバル設定（`~/.gitconfig`）の読み取り
- システム設定（`/etc/gitconfig`）の読み取り
- 設定の優先順位（local > global > system）
- インクルードディレクティブ（`[include]`）対応
- `Config::set()`による設定の書き込み

## 関連Issue

- #016: Repository基本実装（基盤）
- #034: Repository::init()（設定ファイルの初期作成）
