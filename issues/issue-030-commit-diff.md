# Issue #030: コミット変更ファイル一覧（log --stat相当）

## 基本情報

| 項目 | 内容 |
|------|------|
| Phase | 2 |
| 優先度 | 高 |
| 難易度 | 低 |
| 推定時間 | 2h |
| 依存 | #029 |
| 解消するフォールバック | `git log --name-status`, `git log --stat`, `git show` |

## 説明

Commitから直接変更ファイル一覧を取得できるようにする。
#029 Tree Diff を利用して、コミットと親コミットのTree間の差分を計算する。
これにより `git log --name-status` や `git log --stat` 相当の機能を提供。

## 背景

```rust
// 現在の状況
let commit = repo.commit("abc123")?;
// commit.oid(), commit.message() 等は取得可能
// しかし「何が変わったか」は取得できない
// → git log --name-status にフォールバック

// 実装後
for delta in repo.commit_diff(&commit)?.deltas() {
    println!("{} {}", delta.status_char(), delta.path().display());
}
```

## タスク

- [x] `Repository::commit_diff()` を実装
- [x] 親コミットとのTree差分を計算
- [x] 初期コミット（親なし）の処理
- [x] マージコミット（複数親）の処理方針を決定
- [ ] 便利メソッド `Commit::diff()` を追加（オプション）
- [x] 統合テストを作成

## 想定API

```rust
// 基本的な使い方
let commit = repo.commit("abc123")?;
let diff = repo.commit_diff(&commit)?;

println!("Commit: {} {}", commit.oid().short(), commit.summary());
for delta in diff.deltas() {
    println!("  {} {}", delta.status_char(), delta.path().display());
}

// ログと組み合わせ
for commit in repo.log()?.take(5) {
    let commit = commit?;
    let diff = repo.commit_diff(&commit)?;
    
    println!("{} {} (+{} -{} ~{})",
        commit.oid().short(),
        commit.summary(),
        diff.stats().added,
        diff.stats().deleted,
        diff.stats().modified,
    );
}

// git show 風の出力
let commit = repo.commit("abc123")?;
let diff = repo.commit_diff(&commit)?;

println!("commit {}", commit.oid());
println!("Author: {} <{}>", commit.author().name(), commit.author().email());
println!("Date:   {}", format_timestamp(commit.author().time()));
println!();
println!("    {}", commit.message());
println!();
for delta in diff.deltas() {
    println!("{}\t{}", delta.status_char(), delta.path().display());
}
```

## 実装詳細

### メイン実装

```rust
impl Repository {
    /// コミットの変更ファイル一覧を取得
    pub fn commit_diff(&self, commit: &Commit) -> Result<TreeDiff> {
        // 現在のコミットのTree
        let new_tree = self.tree(&commit.tree().to_hex())?;
        
        // 親コミットのTree（なければNone = 初期コミット）
        let old_tree = if let Some(parent_oid) = commit.parent() {
            let parent_commit = self.commit(&parent_oid.to_hex())?;
            Some(self.tree(&parent_commit.tree().to_hex())?)
        } else {
            None
        };
        
        // Tree間の差分を計算
        self.diff_trees(old_tree.as_ref(), &new_tree)
    }
}
```

### マージコミットの処理

マージコミットは複数の親を持つ。処理方針：

```rust
// オプション1: 最初の親との差分のみ（デフォルト、git log --first-parent 相当）
let diff = repo.commit_diff(&commit)?;  // 最初の親との差分

// オプション2: 全親との差分を取得（将来の拡張）
let diffs = repo.commit_diffs_all(&commit)?;  // Vec<TreeDiff>
for (i, diff) in diffs.iter().enumerate() {
    println!("Parent {}: {} files changed", i, diff.deltas().len());
}

// オプション3: combined diff（複雑、将来の拡張）
// 全ての親に対して変更があるファイルのみ表示
```

**現時点の方針**: 最初の親との差分のみを返す（シンプルで十分なケースが多い）

### 初期コミットの処理

```rust
// 親がない場合 = 全ファイルが Added
let diff = repo.diff_trees(None, &tree)?;

// 結果例:
// A README.md
// A src/main.rs
// A Cargo.toml
```

## テストケース

| ID | テスト項目 | 条件 | 期待結果 |
|----|-----------|------|----------|
| CD-001 | 通常コミット | 親が1つ | 親との差分 |
| CD-002 | 初期コミット | 親なし | 全ファイルAdded |
| CD-003 | マージコミット | 親が2つ | 最初の親との差分 |
| CD-004 | 変更なしコミット | 空コミット | 空のdiff |
| CD-005 | 複合変更 | A/D/M混在 | 全て検出 |
| CD-006 | ログ連携 | log() + commit_diff() | 各コミットの差分取得 |

## テストフィクスチャ

```bash
# tests/fixtures/diff リポジトリを利用（#029で作成）

# マージコミットのテスト用に tests/fixtures/merge を作成
# create_fixtures.ps1 に追加済み
```

**作成したフィクスチャ:**
- `tests/fixtures/diff` - 通常コミット、初期コミット、複合変更のテスト
- `tests/fixtures/merge` - マージコミットのテスト（feature ブランチをマージ）

## 出力例

```
$ zerogit log --stat

abc1234 Add new feature
  A src/feature.rs
  M src/lib.rs
  M Cargo.toml
  3 files changed

def5678 Fix bug in parser
  M src/parser.rs
  D src/old_parser.rs
  2 files changed

1112233 Initial commit
  A README.md
  A src/main.rs
  A Cargo.toml
  3 files changed
```

## 受け入れ条件

- [x] 通常のコミットで変更ファイル一覧を取得できる
- [x] 初期コミット（親なし）で全ファイルがAddedとして表示される
- [x] マージコミットで最初の親との差分が取得できる
- [x] `git log --name-status` 相当の出力が得られる
- [x] テスト CD-001〜CD-006 がパス

## 公開API変更

```rust
// Repository に追加
impl Repository {
    pub fn commit_diff(&self, commit: &Commit) -> Result<TreeDiff>;
}
```

## 関連Issue

- #029: Tree Diff実装（この機能の基盤）
- #018: LogIterator実装（組み合わせて使用）
- #028: ログフィルタリング（log --stat的な出力に使用）

## 将来の拡張

- **行数統計**: 各ファイルの追加/削除行数（Blob diffが必要、Phase 3）
- **マージコミットの詳細表示**: combined diff形式
- **グラフィカル表示**: +/- バーの表示
