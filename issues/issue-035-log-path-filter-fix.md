# Issue #035: log_with_options()のパスフィルタリング修正

## 基本情報

| 項目 | 内容 |
|------|------|
| Phase | 2.5 |
| 優先度 | 高 |
| 難易度 | 中 |
| 推定時間 | 2h |
| 依存 | #028 |
| GitHub Issue | https://github.com/siska-tech/zerogit/issues/4 |
| ステータス | 完了 |

## 説明

`log_with_options()`のパスフィルタリングがサブディレクトリ内のファイルを正しく検出できない問題を修正。

## 背景

以前の実装では、以下のようなパスフィルタリングが期待通りに動作しなかった：

```rust
let log = repo.log_with_options(
    LogOptions::new()
        .path("src/main.rs")      // サブディレクトリ内のファイル
        .path("src/")             // ディレクトリ全体
)?;
```

## 問題点

`src/log.rs` の `commit_touches_paths()` メソッドにおいて：

1. **サブディレクトリ内のファイル**: ツリーを再帰的に辿る実装が不完全だった
2. **ディレクトリ全体の変更検出**: ディレクトリパス（例: `src/`）を指定した場合、配下の全ファイルの変更を検出できなかった

## タスク

- [x] `flatten_tree_for_diff()`メソッドを追加（ツリーを再帰的にフラット化）
- [x] `path_matches_filter()`メソッドを追加（パスマッチングロジック）
- [x] `commit_touches_paths()`を修正
- [x] テストケースを追加（LO-011〜LO-013）

## 修正内容

```rust
/// Flattens a tree into a map of path -> OID.
fn flatten_tree_for_diff(&self, tree: &Tree, prefix: PathBuf)
    -> Result<HashMap<PathBuf, Oid>>
{
    let mut result = HashMap::new();
    for entry in tree.entries() {
        let path = prefix.join(entry.name());
        if entry.is_directory() {
            let subtree = self.read_tree(entry.oid())?;
            result.extend(self.flatten_tree_for_diff(&subtree, path)?);
        } else {
            result.insert(path, *entry.oid());
        }
    }
    Ok(result)
}

/// Checks if a path matches any of the configured filter paths.
fn path_matches_filter(&self, path: &Path) -> bool {
    for filter_path in &self.options.paths {
        // Exact match
        if path == filter_path {
            return true;
        }
        // Directory prefix match
        if path.starts_with(filter_path) {
            return true;
        }
    }
    false
}
```

## テストケース

| ID | テスト項目 | 条件 | 期待結果 |
|----|-----------|------|----------|
| LO-011 | サブディレクトリファイル | path("src/lib.rs") | src/lib.rs変更のみ |
| LO-012 | ディレクトリプレフィックス | path("src/") | src/配下の変更のみ |
| LO-013 | スラッシュなしディレクトリ | path("src") | src/配下の変更のみ |

## 受け入れ条件

- [x] `path("src/lib.rs")`でサブディレクトリ内のファイルを正しく検出
- [x] `path("src/")`でディレクトリ配下の全変更を検出
- [x] `path("src")`でもディレクトリとして認識
- [x] テスト LO-011〜LO-013 がパス

## 関連Issue

- #028: ログフィルタリング・オプション対応（基盤）
- #029: Tree Diff実装（フラット化ロジックの参考）
