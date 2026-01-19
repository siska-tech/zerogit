# Issue #028: ログフィルタリング・オプション対応

## 基本情報

| 項目 | 内容 |
|------|------|
| Phase | 1.5 |
| 優先度 | 中 |
| 難易度 | 中 |
| 推定時間 | 4h |
| 依存 | #018 |
| 解消するフォールバック | `git log --path`, `git log -n`, `git log --since` |
| ステータス | ✅ 完了 |

## 説明

log機能を拡張し、パス指定やコミット数制限などのフィルタリングに対応する。
現在の `repo.log()` は全コミットを返すのみで、フィルタリングにはgit CLIへのフォールバックが必要。

## 背景

```rust
// 現在対応済み
repo.log()?;                    // 全履歴
repo.log()?.take(10);           // Rustのイテレータで制限（非効率）

// 未対応（git CLIフォールバックが必要）
git log -- src/main.rs          // 特定ファイルの履歴
git log -n 10                   // 件数制限（内部的に最適化）
git log --since="2024-01-01"    // 日付フィルタ
git log --first-parent          // マージの片側のみ
git log --author="John"         // 作者フィルタ
```

## タスク

- [x] `src/log.rs` に `LogOptions` ビルダーを追加
- [x] `Repository::log_with_options()` を実装
- [x] パスフィルタリングを実装（Tree diffベースで判定）
- [x] 件数制限を実装（内部カウンター）
- [x] 日付範囲フィルタを実装（since/until）
- [x] `--first-parent` 相当を実装
- [x] 作者フィルタを実装（オプション）
- [x] 統合テストを作成

## 想定API

```rust
// ビルダーパターンでオプション指定
let log = repo.log_with_options(
    LogOptions::new()
        .path("src/main.rs")              // 特定ファイルの履歴
        .max_count(10)                     // 最大10件
        .since("2024-01-01")               // 2024年以降
        .until("2024-12-31")               // 2024年まで
        .first_parent(true)                // マージの片側のみ
)?;

for commit in log {
    let commit = commit?;
    println!("{} {}", commit.oid().short(), commit.summary());
}

// 複数パス指定
let log = repo.log_with_options(
    LogOptions::new()
        .paths(&["src/", "Cargo.toml"])
)?;

// 従来のAPIも維持
let log = repo.log()?;  // LogOptions::default() と同等
```

## データ構造

```rust
/// ログ取得オプション
#[derive(Debug, Clone, Default)]
pub struct LogOptions {
    /// フィルタするパス（複数可）
    paths: Vec<PathBuf>,
    /// 最大取得件数
    max_count: Option<usize>,
    /// この日時以降のコミット
    since: Option<i64>,  // Unix timestamp
    /// この日時以前のコミット
    until: Option<i64>,  // Unix timestamp
    /// マージコミットの最初の親のみを辿る
    first_parent: bool,
    /// 作者名でフィルタ（部分一致）
    author: Option<String>,
    /// 開始コミット（デフォルトはHEAD）
    from: Option<Oid>,
}

impl LogOptions {
    pub fn new() -> Self { Self::default() }
    
    pub fn path<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.paths.push(path.as_ref().to_path_buf());
        self
    }
    
    pub fn paths<I, P>(mut self, paths: I) -> Self 
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        self.paths.extend(paths.into_iter().map(|p| p.as_ref().to_path_buf()));
        self
    }
    
    pub fn max_count(mut self, n: usize) -> Self {
        self.max_count = Some(n);
        self
    }
    
    pub fn since(mut self, date: &str) -> Self {
        self.since = Some(parse_date(date));
        self
    }
    
    pub fn until(mut self, date: &str) -> Self {
        self.until = Some(parse_date(date));
        self
    }
    
    pub fn first_parent(mut self, enabled: bool) -> Self {
        self.first_parent = enabled;
        self
    }
    
    pub fn author(mut self, name: &str) -> Self {
        self.author = Some(name.to_string());
        self
    }
    
    pub fn from(mut self, oid: Oid) -> Self {
        self.from = Some(oid);
        self
    }
}
```

## 実装詳細

### パスフィルタリングのアルゴリズム

```
パスフィルタリング:

FOR each commit in history:
    parent_tree = commit.parent()?.tree()  // なければ空Tree
    current_tree = commit.tree()
    
    // 指定パスに関係する変更があるかチェック
    changed = false
    FOR each path in filter_paths:
        old_entry = parent_tree.get_by_path(path)
        new_entry = current_tree.get_by_path(path)
        
        IF old_entry != new_entry:
            changed = true
            BREAK
    
    IF changed:
        YIELD commit
```

### 日付パース

```rust
/// 日付文字列をUnixタイムスタンプに変換
fn parse_date(s: &str) -> i64 {
    // サポート形式:
    // - "2024-01-01"
    // - "2024-01-01T12:00:00"
    // - "1 week ago" (将来対応)
    // - Unix timestamp直接
    
    // 簡易実装: YYYY-MM-DD形式のみ
    // 本格実装ではchronoクレートを検討
}
```

### LogIterator の拡張

```rust
pub struct LogIterator<'a> {
    repo: &'a Repository,
    pending: BinaryHeap<PendingCommit>,
    seen: HashSet<Oid>,
    options: LogOptions,  // 追加
    count: usize,         // 追加: 出力済み件数
}

impl Iterator for LogIterator<'_> {
    type Item = Result<Commit>;
    
    fn next(&mut self) -> Option<Self::Item> {
        // 件数制限チェック
        if let Some(max) = self.options.max_count {
            if self.count >= max {
                return None;
            }
        }
        
        while let Some(pending) = self.pending.pop() {
            // ... 既存のロジック ...
            
            let commit = match self.repo.commit(&oid.to_hex()) {
                Ok(c) => c,
                Err(e) => return Some(Err(e)),
            };
            
            // 日付フィルタ
            if let Some(since) = self.options.since {
                if commit.author().time() < since {
                    continue;  // 古すぎる → スキップ
                }
            }
            if let Some(until) = self.options.until {
                if commit.author().time() > until {
                    continue;  // 新しすぎる → スキップ
                }
            }
            
            // 作者フィルタ
            if let Some(ref author) = self.options.author {
                if !commit.author().name().contains(author) {
                    continue;
                }
            }
            
            // パスフィルタ（要Tree diff）
            if !self.options.paths.is_empty() {
                if !self.commit_touches_paths(&commit)? {
                    continue;
                }
            }
            
            // first-parent: 最初の親のみをキューに追加
            if self.options.first_parent {
                if let Some(parent_oid) = commit.parent() {
                    // 最初の親のみ追加
                }
            } else {
                // 全ての親を追加（既存ロジック）
            }
            
            self.count += 1;
            return Some(Ok(commit));
        }
        
        None
    }
}
```

## テストケース

| ID | テスト項目 | 条件 | 期待結果 |
|----|-----------|------|----------|
| LO-001 | max_count | max_count(5) | 最大5件 |
| LO-002 | path単一 | path("README.md") | 該当ファイル変更のみ |
| LO-003 | path複数 | paths(["src/", "tests/"]) | 該当ディレクトリ変更のみ |
| LO-004 | since | since("2024-01-01") | 指定日以降のみ |
| LO-005 | until | until("2024-06-30") | 指定日以前のみ |
| LO-006 | since + until | 両方指定 | 範囲内のみ |
| LO-007 | first_parent | マージあり | 片側のみ辿る |
| LO-008 | author | author("John") | 該当作者のみ |
| LO-009 | 組み合わせ | 複数オプション | AND条件で適用 |
| LO-010 | 該当なし | 条件に合うコミットなし | 空のイテレータ |

## 受け入れ条件

- [x] 特定ファイルの変更履歴を取得できる
- [x] 件数制限が機能する
- [x] 日付範囲でフィルタできる
- [x] first-parentモードが機能する
- [x] 複数条件の組み合わせが機能する
- [x] テスト LO-001〜LO-010 がパス

## 公開API変更

```rust
// src/lib.rs に追加
pub use log::LogOptions;

// Repository に追加
impl Repository {
    pub fn log_with_options(&self, options: LogOptions) -> Result<LogIterator<'_>>;
}
```

## 注意事項

### パフォーマンス

パスフィルタリングは各コミットでTree比較が必要なため、大規模リポジトリでは遅くなる可能性がある。
最適化案：
1. パスのプレフィックスでTree走査を限定
2. キャッシュの活用
3. 並列処理（将来）

### Tree Diff との関係

パスフィルタリングの完全実装には #029 Tree Diff が必要。
暫定実装として、指定パスのエントリOID比較のみで判定することも可能。

## 関連Issue

- #018: LogIterator実装（基盤）
- #029: Tree Diff実装（パスフィルタの完全実装に必要）
