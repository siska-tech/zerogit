# Issue #029: Tree Diff実装

## 基本情報

| 項目 | 内容 |
|------|------|
| Phase | 2 |
| 優先度 | **高（最重要）** |
| 難易度 | 中 |
| 推定時間 | 5h |
| 依存 | #010, #017 |
| 解消するフォールバック | `git diff --name-status`, `git log --stat` の基盤 |
| **ステータス** | **完了** |

## 説明

2つのTree間の差分を計算する機能を実装。これはdiff機能全体の基盤であり、以下の機能に必要：
- コミットの変更ファイル一覧（`git log --stat`）
- ワーキングツリーの差分（`git diff`）
- ステージングの差分（`git diff --staged`）

**これが最も優先度の高いIssue。** これを実装すれば、log詳細とdiffの両方のフォールバックを解消できる。

## 背景

```rust
// 現在の状況
// Tree間の差分を取得する方法がない
// → git diff --name-status にフォールバック

// 実装後
let diff = repo.diff_trees(&old_tree, &new_tree)?;
for delta in diff.deltas() {
    println!("{} {}", delta.status_char(), delta.path().display());
}
```

## タスク

- [x] `src/diff/mod.rs` を作成
- [x] `TreeDiff` 構造体を定義
- [x] `DiffDelta` 構造体を定義
- [x] `DiffStatus` enumを定義
- [x] Treeフラット化ユーティリティを実装
- [x] `diff_trees()` コア関数を実装
- [x] `Repository::diff_trees()` を実装
- [x] リネーム検出を実装（オプション）
- [x] ユニットテストを作成
- [x] 統合テストを作成

## 想定API

```rust
// 基本的な使い方
let old_tree = repo.tree(&old_oid.to_hex())?;
let new_tree = repo.tree(&new_oid.to_hex())?;
let diff = repo.diff_trees(Some(&old_tree), &new_tree)?;

for delta in diff.deltas() {
    let status = match delta.status() {
        DiffStatus::Added => "A",
        DiffStatus::Deleted => "D",
        DiffStatus::Modified => "M",
        DiffStatus::Renamed => "R",
        DiffStatus::Copied => "C",
    };
    println!("{} {}", status, delta.path().display());
}

// 初期コミット（親なし）の場合
let diff = repo.diff_trees(None, &tree)?;  // old_tree = None

// 変更されたファイル数
println!("Files changed: {}", diff.deltas().len());

// ステータス別にカウント
let stats = diff.stats();
println!("+{} -{} ~{}", stats.added, stats.deleted, stats.modified);
```

## データ構造

```rust
/// Tree間の差分
#[derive(Debug, Clone)]
pub struct TreeDiff {
    deltas: Vec<DiffDelta>,
}

impl TreeDiff {
    pub fn deltas(&self) -> &[DiffDelta] {
        &self.deltas
    }
    
    pub fn stats(&self) -> DiffStats {
        let mut stats = DiffStats::default();
        for delta in &self.deltas {
            match delta.status {
                DiffStatus::Added => stats.added += 1,
                DiffStatus::Deleted => stats.deleted += 1,
                DiffStatus::Modified => stats.modified += 1,
                DiffStatus::Renamed => stats.renamed += 1,
                DiffStatus::Copied => stats.copied += 1,
            }
        }
        stats
    }
    
    pub fn is_empty(&self) -> bool {
        self.deltas.is_empty()
    }
}

/// 差分の統計
#[derive(Debug, Clone, Default)]
pub struct DiffStats {
    pub added: usize,
    pub deleted: usize,
    pub modified: usize,
    pub renamed: usize,
    pub copied: usize,
}

/// 差分の各エントリ
#[derive(Debug, Clone)]
pub struct DiffDelta {
    /// 変更の種類
    status: DiffStatus,
    /// ファイルパス（新しい方、またはリネーム後）
    path: PathBuf,
    /// リネーム/コピー元のパス
    old_path: Option<PathBuf>,
    /// 変更前のOID（Deletedの場合やModifiedの旧版）
    old_oid: Option<Oid>,
    /// 変更後のOID（Addedの場合やModifiedの新版）
    new_oid: Option<Oid>,
    /// 変更前のファイルモード
    old_mode: Option<FileMode>,
    /// 変更後のファイルモード
    new_mode: Option<FileMode>,
}

impl DiffDelta {
    pub fn status(&self) -> DiffStatus { self.status }
    pub fn path(&self) -> &Path { &self.path }
    pub fn old_path(&self) -> Option<&Path> { self.old_path.as_deref() }
    pub fn old_oid(&self) -> Option<&Oid> { self.old_oid.as_ref() }
    pub fn new_oid(&self) -> Option<&Oid> { self.new_oid.as_ref() }
    
    /// git status 形式の1文字ステータス
    pub fn status_char(&self) -> char {
        match self.status {
            DiffStatus::Added => 'A',
            DiffStatus::Deleted => 'D',
            DiffStatus::Modified => 'M',
            DiffStatus::Renamed => 'R',
            DiffStatus::Copied => 'C',
        }
    }
}

/// 差分のステータス
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffStatus {
    /// 新規追加
    Added,
    /// 削除
    Deleted,
    /// 変更
    Modified,
    /// リネーム
    Renamed,
    /// コピー
    Copied,
}
```

## アルゴリズム

### 基本アルゴリズム

```
Tree Diff アルゴリズム:

入力: old_tree (Option<Tree>), new_tree (Tree)
出力: Vec<DiffDelta>

1. 両方のTreeをフラット化
   old_map: HashMap<PathBuf, (Oid, FileMode)> = flatten(old_tree)
   new_map: HashMap<PathBuf, (Oid, FileMode)> = flatten(new_tree)

2. 全パスの和集合を取得
   all_paths = old_map.keys() ∪ new_map.keys()

3. 各パスを比較
   deltas = []
   FOR path in all_paths:
       old_entry = old_map.get(path)
       new_entry = new_map.get(path)
       
       MATCH (old_entry, new_entry):
           (None, Some(new)):
               deltas.push(DiffDelta::added(path, new))
           
           (Some(old), None):
               deltas.push(DiffDelta::deleted(path, old))
           
           (Some(old), Some(new)) where old.oid != new.oid:
               deltas.push(DiffDelta::modified(path, old, new))
           
           (Some(old), Some(new)) where old.oid == new.oid:
               // 変更なし、スキップ
               // ただしモード変更は検出可能（オプション）

4. リネーム検出（オプション）
   detect_renames(&mut deltas)

5. パスでソートして返却
   deltas.sort_by(|a, b| a.path.cmp(&b.path))
   RETURN deltas
```

### Treeフラット化

```
flatten(tree: &Tree, prefix: PathBuf) -> HashMap<PathBuf, (Oid, FileMode)>:

result = HashMap::new()

FOR entry in tree.entries():
    path = prefix.join(entry.name())
    
    IF entry.is_tree():
        subtree = repo.tree(entry.oid())?
        result.extend(flatten(subtree, path))
    ELSE:
        result.insert(path, (entry.oid(), entry.mode()))

RETURN result
```

### リネーム検出（オプション）

```
detect_renames(deltas: &mut Vec<DiffDelta>):

// Deleted と Added をペアリング
deleted: Vec<&DiffDelta> = deltas.filter(|d| d.status == Deleted)
added: Vec<&DiffDelta> = deltas.filter(|d| d.status == Added)

FOR del in deleted:
    FOR add in added:
        // 同じOID = 完全一致のリネーム
        IF del.old_oid == add.new_oid:
            // del と add を削除し、Renamed として追加
            mark_as_rename(del, add)
            BREAK
        
        // 類似度ベースのリネーム検出（将来）
        // similarity = compute_similarity(del.content, add.content)
        // IF similarity > 0.5:
        //     mark_as_rename(del, add)
```

## 実装コード例

```rust
impl Repository {
    pub fn diff_trees(
        &self,
        old_tree: Option<&Tree>,
        new_tree: &Tree,
    ) -> Result<TreeDiff> {
        // フラット化
        let old_map = match old_tree {
            Some(tree) => self.flatten_tree(tree, PathBuf::new())?,
            None => HashMap::new(),
        };
        let new_map = self.flatten_tree(new_tree, PathBuf::new())?;
        
        // 全パス収集
        let mut all_paths: BTreeSet<PathBuf> = BTreeSet::new();
        all_paths.extend(old_map.keys().cloned());
        all_paths.extend(new_map.keys().cloned());
        
        // 比較
        let mut deltas = Vec::new();
        for path in all_paths {
            let old_entry = old_map.get(&path);
            let new_entry = new_map.get(&path);
            
            match (old_entry, new_entry) {
                (None, Some((oid, mode))) => {
                    deltas.push(DiffDelta {
                        status: DiffStatus::Added,
                        path: path.clone(),
                        old_path: None,
                        old_oid: None,
                        new_oid: Some(*oid),
                        old_mode: None,
                        new_mode: Some(*mode),
                    });
                }
                (Some((oid, mode)), None) => {
                    deltas.push(DiffDelta {
                        status: DiffStatus::Deleted,
                        path: path.clone(),
                        old_path: None,
                        old_oid: Some(*oid),
                        new_oid: None,
                        old_mode: Some(*mode),
                        new_mode: None,
                    });
                }
                (Some((old_oid, old_mode)), Some((new_oid, new_mode))) => {
                    if old_oid != new_oid {
                        deltas.push(DiffDelta {
                            status: DiffStatus::Modified,
                            path: path.clone(),
                            old_path: None,
                            old_oid: Some(*old_oid),
                            new_oid: Some(*new_oid),
                            old_mode: Some(*old_mode),
                            new_mode: Some(*new_mode),
                        });
                    }
                    // OIDが同じ = 変更なし
                }
                (None, None) => unreachable!(),
            }
        }
        
        Ok(TreeDiff { deltas })
    }
}
```

## テストケース

| ID | テスト項目 | 条件 | 期待結果 |
|----|-----------|------|----------|
| TD-001 | 追加検出 | new_treeにのみ存在 | DiffStatus::Added |
| TD-002 | 削除検出 | old_treeにのみ存在 | DiffStatus::Deleted |
| TD-003 | 変更検出 | 両方に存在、OID異なる | DiffStatus::Modified |
| TD-004 | 変更なし | 両方に存在、OID同じ | deltaなし |
| TD-005 | 空Tree比較 | old_tree = None | 全ファイルAdded |
| TD-006 | 同一Tree | 同じTree同士 | 空のdiff |
| TD-007 | ネストしたパス | src/lib.rs等 | 正しいパス |
| TD-008 | 複数変更 | A, D, M混在 | 全て検出 |
| TD-009 | リネーム | 同じOIDで別パス | DiffStatus::Renamed |
| TD-010 | stats | 各種変更 | 正しいカウント |

## テストフィクスチャ追加

```bash
# tests/fixtures/create_fixtures.sh に追加

# diff: 差分テスト用リポジトリ
rm -rf diff
mkdir -p diff && cd diff
git init
git config user.email "test@example.com"
git config user.name "Test User"

# 初期コミット
echo "initial" > file1.txt
echo "to-delete" > file2.txt
mkdir -p src
echo "fn main() {}" > src/main.rs
git add .
git commit -m "Initial commit"

# 変更コミット
echo "modified" > file1.txt          # Modified
rm file2.txt                          # Deleted
echo "new file" > file3.txt           # Added
echo "fn main() { println!(\"hello\"); }" > src/main.rs  # Modified
git add .
git commit -m "Various changes"

cd ..
```

## 受け入れ条件

- [x] 追加・削除・変更ファイルを正しく検出できる
- [x] `git diff --name-status` 相当の出力が得られる
- [x] 空Tree（初期コミット）との比較ができる
- [x] ネストしたディレクトリ構造を正しく処理
- [x] パスでソートされた結果が返る
- [x] テスト TD-001〜TD-010 がパス

## 公開API変更

```rust
// src/lib.rs に追加
pub use diff::{TreeDiff, DiffDelta, DiffStatus, DiffStats};

// Repository に追加
impl Repository {
    pub fn diff_trees(
        &self,
        old_tree: Option<&Tree>,
        new_tree: &Tree,
    ) -> Result<TreeDiff>;
}
```

## 関連Issue

- #010: Treeパース実装（基盤）
- #017: Repositoryオブジェクト取得（基盤）
- #030: コミット変更ファイル一覧（この機能を利用）
- #031: Working Tree / Index Diff（この機能を拡張）
- #028: ログフィルタリング（パスフィルタにこの機能を利用）

## 将来の拡張

- **類似度ベースのリネーム検出**: Blob内容の類似度でリネーム判定
- **コピー検出**: 同一内容の新規ファイルを検出
- **サブモジュール対応**: サブモジュールの変更を検出
- **バイナリファイル判定**: バイナリ変更のフラグ

## 実装ノート

### 実装したファイル

- `src/diff/mod.rs` - Tree Diff機能のメイン実装
- `src/lib.rs` - 公開API追加 (`DiffDelta`, `DiffStats`, `DiffStatus`, `TreeDiff`)
- `tests/diff_test.rs` - 統合テスト（14テストケース）
- `tests/fixtures/diff/` - 差分テスト用フィクスチャ
- `tests/fixtures/rename/` - リネーム検出テスト用フィクスチャ
- `tests/fixtures/create_fixtures.ps1` - フィクスチャ作成スクリプト更新

### テスト結果

```
running 14 tests
test test_td001_detect_added ... ok
test test_td002_detect_deleted ... ok
test test_td003_detect_modified ... ok
test test_td004_same_tree_no_changes ... ok
test test_td005_empty_old_tree ... ok
test test_td006_identical_trees_empty_diff ... ok
test test_td007_nested_paths ... ok
test test_td008_multiple_changes ... ok
test test_td009_rename_detection ... ok
test test_td010_stats_calculation ... ok
test test_deltas_sorted_by_path ... ok
test test_iterator_support ... ok
test test_oid_accessors ... ok
test test_status_char ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

### 追加機能

- **完全一致リネーム検出**: 同一OIDを持つDeleted/Addedペアを自動的にRenamedとして検出
- **IntoIterator実装**: `TreeDiff`と`&TreeDiff`の両方でfor-in構文が使用可能
- **モード変更検出**: `old_mode`/`new_mode`フィールドでファイルモードの変更を追跡
