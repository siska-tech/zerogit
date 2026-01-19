# Issue #033: Repository::branches()メソッドを追加

## 基本情報

| 項目 | 内容 |
|------|------|
| Phase | 2.5 |
| 優先度 | 中 |
| 難易度 | 低 |
| 推定時間 | 1h |
| 依存 | #014, #027 |
| GitHub Issue | https://github.com/siska-tech/zerogit/issues/2 |
| ステータス | 完了 |

## 説明

`Repository`にローカルブランチ一覧を取得する`branches()`メソッドを追加する。

## 背景

以前、ブランチ関連のAPIは以下のみが公開されていた：

- `Repository::create_branch()` - ブランチ作成
- `Repository::delete_branch()` - ブランチ削除
- `Repository::remote_branches()` - リモートブランチ一覧

ローカルブランチの一覧を取得する`branches()`メソッドがなかったため、内部の`RefStore::branches()`に直接アクセスする必要があったが、`RefStore`は公開APIではなかった。

## タスク

- [x] `Repository::branches()`を実装
- [x] `Vec<Branch>`として返却
- [x] `remote_branches()`と対称的なAPIを提供
- [x] テストを作成

## 想定API

```rust
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

// リモートブランチ（既存API）
for rb in repo.remote_branches()? {
    println!("  remotes/{}/{}", rb.remote(), rb.name());
}
```

## 実装

```rust
impl Repository {
    /// Returns a list of all local branches.
    pub fn branches(&self) -> Result<Vec<Branch>> {
        self.ref_store().branches()
            .map(|names| names.into_iter()
                .filter_map(|name| self.resolve_branch(&name).ok())
                .collect())
    }
}
```

## 受け入れ条件

- [x] `Repository::branches()`でローカルブランチ一覧を取得できる
- [x] `Vec<Branch>`として返却される
- [x] `remote_branches()`と対称的なAPIになっている

## 関連Issue

- #014: Head/Branch型実装（基盤）
- #027: リモートブランチ・タグ対応（対称的なAPI）
