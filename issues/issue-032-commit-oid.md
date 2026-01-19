# Issue #032: Commit構造体にoid()メソッドを追加

## 基本情報

| 項目 | 内容 |
|------|------|
| Phase | 2.5 |
| 優先度 | 高 |
| 難易度 | 低 |
| 推定時間 | 1h |
| 依存 | #011 |
| GitHub Issue | https://github.com/siska-tech/zerogit/issues/1 |
| ステータス | 完了 |

## 説明

`Commit`構造体に自身のOIDを取得する`oid()`メソッドを追加する。

## 背景

現在、`Commit`構造体は以下のメソッドを公開しているが、コミット自身のOIDを取得する方法がなかった：

- `tree()` - ツリーのOID
- `parents()` / `parent()` - 親コミットのOID
- `author()` / `committer()` - 署名情報
- `message()` / `summary()` - コミットメッセージ

git logなどの機能を実装する際、コミットハッシュの表示が必要だが、以前は取得できないためgit CLIにフォールバックする必要があった。

## タスク

- [x] `Commit`構造体に`oid`フィールドを追加
- [x] `Commit::oid()`メソッドを実装
- [x] `Commit::parse()`に`oid`パラメータを追加
- [x] `Repository::log()`の戻り値でOIDを設定
- [x] `Oid::short()`メソッドを追加（7文字の短縮形式）

## 想定API

```rust
let commit = repo.commit("abc1234")?;
println!("Commit: {}", commit.oid());        // 完全なOID
println!("Short:  {}", commit.oid().short()); // 7文字の短縮形式

for commit in repo.log()?.take(10) {
    let commit = commit?;
    println!("{} {}", commit.oid().short(), commit.summary());
}
```

## 実装

```rust
impl Commit {
    /// Returns the OID of this commit.
    pub fn oid(&self) -> &Oid {
        &self.oid
    }
}

impl Oid {
    /// Returns a short (7-character) representation of this OID.
    pub fn short(&self) -> String {
        self.to_hex()[..7].to_string()
    }
}
```

## 受け入れ条件

- [x] `Commit::oid()`でコミットのOIDを取得できる
- [x] `Oid::short()`で7文字の短縮形式を取得できる
- [x] `Repository::log()`のイテレータで各コミットのOIDが取得可能

## 関連Issue

- #011: Commitパース実装（基盤）
- #018: LogIterator実装（この機能を利用）
