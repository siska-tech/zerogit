# Issue #031: Working Tree / Index Diff

## 基本情報

| 項目 | 内容 |
|------|------|
| Phase | 2 |
| 優先度 | 中 |
| 難易度 | 中 |
| 推定時間 | 3h |
| 依存 | #015, #029 |
| 解消するフォールバック | `git diff`, `git diff --staged`, `git diff HEAD` |

## 説明

ワーキングツリーとIndex、IndexとHEADの差分を取得する機能。
これにより以下のgitコマンド相当の機能を提供：
- `git diff` → ワーキングツリー vs Index（未ステージの変更）
- `git diff --staged` / `git diff --cached` → Index vs HEAD（ステージ済みの変更）
- `git diff HEAD` → ワーキングツリー vs HEAD（全変更）

## 背景

```rust
// 現在の状況
let status = repo.status()?;
// FileStatus::Modified 等は分かるが、具体的な差分は取得できない
// → git diff にフォールバック

// 実装後
let unstaged = repo.diff_index_to_workdir()?;
let staged = repo.diff_head_to_index()?;
```

## タスク

- [x] `Repository::diff_index_to_workdir()` を実装
- [x] `Repository::diff_head_to_index()` を実装
- [x] `Repository::diff_head_to_workdir()` を実装
- [x] ワーキングツリーファイルのハッシュ計算
- [x] 仮想Tree（Index/Workdirから）の構築
- [x] `status()` との整合性を確保
- [x] 統合テストを作成

## 想定API

```rust
// git diff 相当（ワーキングツリー vs Index）
let unstaged = repo.diff_index_to_workdir()?;
println!("Unstaged changes:");
for delta in unstaged.deltas() {
    println!("  {} {}", delta.status_char(), delta.path().display());
}

// git diff --staged 相当（Index vs HEAD）
let staged = repo.diff_head_to_index()?;
println!("Staged changes:");
for delta in staged.deltas() {
    println!("  {} {}", delta.status_char(), delta.path().display());
}

// git diff HEAD 相当（ワーキングツリー vs HEAD）
let all_changes = repo.diff_head_to_workdir()?;
println!("All changes from HEAD:");
for delta in all_changes.deltas() {
    println!("  {} {}", delta.status_char(), delta.path().display());
}

// status() との比較
let status = repo.status()?;
let unstaged = repo.diff_index_to_workdir()?;
let staged = repo.diff_head_to_index()?;

// status の Modified/Deleted = unstaged の M/D
// status の StagedModified/Added/StagedDeleted = staged の M/A/D
```

## 実装詳細

### アーキテクチャ

```
                     diff_head_to_index()
          ┌─────────────────────────────────────┐
          │                                     │
          v                                     │
     ┌─────────┐                          ┌─────────┐
     │  HEAD   │                          │  Index  │
     │  Tree   │                          │         │
     └─────────┘                          └─────────┘
          │                                     │
          │      diff_head_to_workdir()         │
          │ ┌─────────────────────────────────┐ │
          │ │                                 │ │
          v v                                 v v
     ┌─────────────────────────────────────────────┐
     │              Working Tree                   │
     └─────────────────────────────────────────────┘
                          ^
                          │
          diff_index_to_workdir()
```

### Index → 仮想Tree変換

```rust
/// IndexエントリをTree風のマップに変換
fn index_to_tree_map(index: &Index) -> HashMap<PathBuf, (Oid, FileMode)> {
    index.entries()
        .iter()
        .map(|e| (e.path().to_path_buf(), (*e.oid(), e.mode())))
        .collect()
}
```

### Working Tree → 仮想Tree変換

```rust
/// ワーキングツリーを走査してTree風のマップを構築
fn workdir_to_tree_map(
    repo: &Repository,
    index: &Index,
) -> Result<HashMap<PathBuf, (Oid, FileMode)>> {
    let mut map = HashMap::new();
    
    for file in list_working_tree(repo.path())? {
        let rel_path = file.strip_prefix(repo.path())?;
        
        // ファイル内容をハッシュ
        let content = fs::read(&file)?;
        let oid = hash::hash_object("blob", &content);
        
        // モードはIndexから取得、なければ推測
        let mode = index.get(rel_path)
            .map(|e| e.mode())
            .unwrap_or_else(|| detect_file_mode(&file));
        
        map.insert(rel_path.to_path_buf(), (oid, mode));
    }
    
    Ok(map)
}

/// ファイルモードを推測
fn detect_file_mode(path: &Path) -> FileMode {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path).ok();
        if let Some(m) = meta {
            if m.permissions().mode() & 0o111 != 0 {
                return FileMode::Executable;
            }
        }
    }
    FileMode::Regular
}
```

### メイン実装

```rust
impl Repository {
    /// git diff 相当（Index vs Working Tree）
    pub fn diff_index_to_workdir(&self) -> Result<TreeDiff> {
        let index = self.index()?;
        let index_map = index_to_tree_map(&index);
        let workdir_map = workdir_to_tree_map(self, &index)?;
        
        diff_maps(&index_map, &workdir_map)
    }
    
    /// git diff --staged 相当（HEAD vs Index）
    pub fn diff_head_to_index(&self) -> Result<TreeDiff> {
        let head_tree = self.get_head_tree()?;
        let head_map = self.flatten_tree(&head_tree, PathBuf::new())?;
        
        let index = self.index()?;
        let index_map = index_to_tree_map(&index);
        
        diff_maps(&head_map, &index_map)
    }
    
    /// git diff HEAD 相当（HEAD vs Working Tree）
    pub fn diff_head_to_workdir(&self) -> Result<TreeDiff> {
        let head_tree = self.get_head_tree()?;
        let head_map = self.flatten_tree(&head_tree, PathBuf::new())?;
        
        let index = self.index()?;
        let workdir_map = workdir_to_tree_map(self, &index)?;
        
        diff_maps(&head_map, &workdir_map)
    }
    
    /// HEADのTreeを取得
    fn get_head_tree(&self) -> Result<Tree> {
        let head = self.head()?;
        let commit = self.commit(&head.oid().to_hex())?;
        self.tree(&commit.tree().to_hex())
    }
}

/// 2つのマップ間の差分を計算
fn diff_maps(
    old_map: &HashMap<PathBuf, (Oid, FileMode)>,
    new_map: &HashMap<PathBuf, (Oid, FileMode)>,
) -> Result<TreeDiff> {
    // #029 Tree Diff と同じロジック
    let mut all_paths: BTreeSet<PathBuf> = BTreeSet::new();
    all_paths.extend(old_map.keys().cloned());
    all_paths.extend(new_map.keys().cloned());
    
    let mut deltas = Vec::new();
    for path in all_paths {
        // ... 比較ロジック（#029と同様）
    }
    
    Ok(TreeDiff { deltas })
}
```

## status() との関係

```rust
// status() の結果と diff の対応関係
FileStatus::Untracked     → diff_index_to_workdir() の Added（Indexにない）
FileStatus::Modified      → diff_index_to_workdir() の Modified
FileStatus::Deleted       → diff_index_to_workdir() の Deleted
FileStatus::Added         → diff_head_to_index() の Added
FileStatus::StagedModified → diff_head_to_index() の Modified
FileStatus::StagedDeleted  → diff_head_to_index() の Deleted
```

## テストケース

| ID | テスト項目 | 条件 | 期待結果 |
|----|-----------|------|----------|
| WD-001 | 未ステージ変更 | ファイル編集後 | diff_index_to_workdir に Modified |
| WD-002 | 未ステージ削除 | ファイル削除後 | diff_index_to_workdir に Deleted |
| WD-003 | 未追跡ファイル | 新規ファイル | diff_index_to_workdir に Added |
| WD-004 | ステージ済み変更 | git add 後 | diff_head_to_index に Modified |
| WD-005 | ステージ済み追加 | 新規 + git add | diff_head_to_index に Added |
| WD-006 | クリーン状態 | 変更なし | 全diff が空 |
| WD-007 | HEAD diff | 複合変更 | staged + unstaged の合計 |
| WD-008 | status整合 | 任意の状態 | status() と結果が一致 |

## テスト用ヘルパー

```rust
#[test]
fn test_diff_consistency_with_status() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    
    // テスト用ファイル作成
    let test_file = repo.path().join("test.txt");
    std::fs::write(&test_file, "test content").unwrap();
    
    // status と diff の結果を比較
    let status = repo.status().unwrap();
    let unstaged = repo.diff_index_to_workdir().unwrap();
    
    // Untracked は diff では Added として現れる
    let untracked_count = status.iter()
        .filter(|e| e.status() == FileStatus::Untracked)
        .count();
    let diff_added_count = unstaged.deltas().iter()
        .filter(|d| d.status() == DiffStatus::Added)
        .count();
    
    // Untracked は Indexにないファイル = diff_index_to_workdir では Added
    assert!(diff_added_count >= untracked_count);
    
    // クリーンアップ
    std::fs::remove_file(test_file).ok();
}
```

## 受け入れ条件

- [x] `git diff` 相当（未ステージ変更）が取得できる
- [x] `git diff --staged` 相当（ステージ済み変更）が取得できる
- [x] `git diff HEAD` 相当（全変更）が取得できる
- [x] `status()` の結果と整合性がある
- [ ] パフォーマンスが許容範囲（10000ファイルで < 1秒）
- [x] テスト WD-001〜WD-008 がパス

## 公開API変更

```rust
// Repository に追加
impl Repository {
    /// git diff 相当（Index vs Working Tree）
    pub fn diff_index_to_workdir(&self) -> Result<TreeDiff>;
    
    /// git diff --staged 相当（HEAD vs Index）
    pub fn diff_head_to_index(&self) -> Result<TreeDiff>;
    
    /// git diff HEAD 相当（HEAD vs Working Tree）
    pub fn diff_head_to_workdir(&self) -> Result<TreeDiff>;
}
```

## パフォーマンス考慮

### ハッシュ計算の最適化

```rust
// 全ファイルをハッシュするのは重い
// → mtime/サイズでスキップ可能なファイルを判定

fn workdir_to_tree_map_optimized(
    repo: &Repository,
    index: &Index,
) -> Result<HashMap<PathBuf, (Oid, FileMode)>> {
    let mut map = HashMap::new();
    
    for file in list_working_tree(repo.path())? {
        let rel_path = file.strip_prefix(repo.path())?;
        let metadata = std::fs::metadata(&file)?;
        
        // Indexに同じパスがあり、mtime/サイズが同じならハッシュをスキップ
        if let Some(index_entry) = index.get(rel_path) {
            let mtime = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_secs();
            if mtime == index_entry.mtime() && 
               metadata.len() as u32 == index_entry.size() {
                // 変更なしと判定、Indexの値を使用
                map.insert(rel_path.to_path_buf(), 
                          (*index_entry.oid(), index_entry.mode()));
                continue;
            }
        }
        
        // 実際にハッシュ計算
        let content = fs::read(&file)?;
        let oid = hash::hash_object("blob", &content);
        // ...
    }
    
    Ok(map)
}
```

## 関連Issue

- #015: Indexパース実装（Indexの読み取り）
- #019: Status実装（類似機能、整合性確認）
- #029: Tree Diff実装（基盤アルゴリズム）

## 将来の拡張

- **パス指定diff**: `diff_index_to_workdir_path("src/")` 特定ディレクトリのみ
- **バイナリ判定**: バイナリファイルの変更フラグ
- **行単位diff**: Blob diffとの連携（Phase 3）

## 実装ノート

### 実装された機能

`src/diff/mod.rs` に以下の3つのメソッドを実装:

1. **`diff_index_to_workdir()`** - Index とワーキングツリー間の差分を計算
2. **`diff_head_to_index()`** - HEAD と Index 間の差分を計算
3. **`diff_head_to_workdir()`** - HEAD とワーキングツリー間の差分を計算

### 主要な実装詳細

- `FlatEntry` 構造体（oid, mode）を使用してツリー/Index/ワーキングツリーを共通のフォーマットで表現
- `index_to_flat_map()`: Index を FlatEntry マップに変換
- `workdir_to_flat_map()`: ワーキングツリーを走査し、各ファイルをハッシュ化して FlatEntry マップを構築
- `diff_flat_maps()`: 2つの FlatEntry マップ間の差分を計算（既存の TreeDiff 構造を再利用）
- `normalize_path()`: クロスプラットフォーム対応のためパス区切り文字を統一

### テスト

`tests/diff_test.rs` に以下のテストケースを追加:
- WD-001: 未ステージ変更の検出
- WD-002: 未ステージ削除の検出
- WD-003: 未追跡ファイルの検出（Added として表示）
- WD-004: ステージ済み変更の検出
- WD-005: ステージ済み追加の検出
- WD-006: クリーン状態（全 diff が空）
- WD-007: HEAD diff（staged + unstaged の合計）
- WD-008: status() との整合性確認

### 注意点

- 改行コード（CRLF/LF）の違いによりハッシュが変わるため、テスト時は `core.autocrlf=false` を設定しフィクスチャファイルをLFに統一する必要がある
- パフォーマンス最適化（mtime/サイズによるスキップ）は将来の拡張として残している
