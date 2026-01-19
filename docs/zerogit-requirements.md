# zerogit 要件定義書

## 1. 開発背景と目的

### 1.1 解決しようとしている課題

既存のGitライブラリには以下の課題がある：

| ライブラリ | 課題                                            |
| ---------- | ----------------------------------------------- |
| git2-rs    | libgit2（C言語）へのFFI依存。ビルド環境の複雑化 |
| gitoxide   | 高機能だが多数のcrateに分割され依存が多い       |

Markdownベースのプロジェクト管理ツールを開発するにあたり、軽量かつPure RustなGitクライアントライブラリが必要となった。

### 1.2 想定される利用者（ターゲットユーザー）

- Markdownベースプロジェクト管理ツールの開発者（自プロジェクト）
- 軽量なGit操作を必要とするRustアプリケーション開発者
- Cバインディングを避けたいRust開発者

### 1.3 提供する中核的な価値

- **軽量性**: 最小限の依存で導入可能
- **Pure Rust**: Cコンパイラ不要、クロスコンパイル容易
- **学習可能性**: シンプルな実装でGit内部構造の理解に貢献

---

## 2. 機能要件

### Phase 1: リポジトリ読み取り（MVP）

#### 2.1.1 リポジトリ検出

| 機能         | 入力                 | 出力                 |
| ------------ | -------------------- | -------------------- |
| `discover()` | カレントディレクトリ | `Result<Repository>` |
| `open(path)` | パス文字列           | `Result<Repository>` |
| `is_valid()` | -                    | `bool`               |

#### 2.1.2 オブジェクト読み取り

Gitオブジェクト形式（zlib圧縮）のパース。

| オブジェクト | 入力          | 出力                                             |
| ------------ | ------------- | ------------------------------------------------ |
| blob         | SHA-1ハッシュ | ファイル内容（バイト列）                         |
| tree         | SHA-1ハッシュ | エントリ一覧（モード、名前、SHA-1）              |
| commit       | SHA-1ハッシュ | author, committer, message, parent, tree         |
| tag          | SHA-1ハッシュ | 対象オブジェクト、タグ名、メッセージ（後回し可） |

#### 2.1.3 参照（refs）読み取り

| 機能             | 入力   | 出力                        |
| ---------------- | ------ | --------------------------- |
| HEAD解決         | -      | ブランチ名 or コミットSHA-1 |
| ブランチ一覧     | -      | `Vec<Branch>`               |
| タグ一覧         | -      | `Vec<Tag>`                  |
| symbolic-ref解決 | 参照名 | 最終的なSHA-1               |

#### 2.1.4 コミット履歴

| 機能               | 入力                       | 出力               |
| ------------------ | -------------------------- | ------------------ |
| `log()`            | オプション（件数、開始点） | `Iterator<Commit>` |
| `commit_info(sha)` | SHA-1                      | `Commit` 構造体    |

#### 2.1.5 ワーキングツリー状態

| 機能       | 入力 | 出力               |
| ---------- | ---- | ------------------ |
| `status()` | -    | `Vec<StatusEntry>` |

StatusEntryの状態：

| 状態      | 説明                   |
| --------- | ---------------------- |
| Untracked | Git管理外ファイル      |
| Modified  | 変更あり（未ステージ） |
| Staged    | ステージ済み           |
| Deleted   | 削除された             |
| Clean     | 変更なし               |

### Phase 2: 書き込み操作

#### 2.2.1 ステージング

| 機能          | 入力         | 出力         |
| ------------- | ------------ | ------------ |
| `add(path)`   | ファイルパス | `Result<()>` |
| `add_all()`   | -            | `Result<()>` |
| `reset(path)` | ファイルパス | `Result<()>` |

#### 2.2.2 コミット

| 機能              | 入力               | 出力          |
| ----------------- | ------------------ | ------------- |
| `commit(message)` | コミットメッセージ | `Result<Oid>` |

#### 2.2.3 ブランチ操作

| 機能                  | 入力       | 出力             |
| --------------------- | ---------- | ---------------- |
| `branch_create(name)` | ブランチ名 | `Result<Branch>` |
| `branch_delete(name)` | ブランチ名 | `Result<()>`     |
| `checkout(branch)`    | ブランチ名 | `Result<()>`     |

### Phase 2.5: 拡張読み取り機能（✅ 完了）

実際の利用で必要となった追加機能。

#### 2.2.5.1 リポジトリ初期化

| 機能           | 入力       | 出力                 |
| -------------- | ---------- | -------------------- |
| `init(path)`   | ディレクトリパス | `Result<Repository>` |

#### 2.2.5.2 リモートブランチ・タグ

| 機能               | 入力 | 出力                     |
| ------------------ | ---- | ------------------------ |
| `remote_branches()` | -    | `Result<Vec<RemoteBranch>>` |
| `tags()`           | -    | `Result<Vec<Tag>>`       |

#### 2.2.5.3 ログフィルタリング

| 機能                         | 入力           | 出力                 |
| ---------------------------- | -------------- | -------------------- |
| `log_with_options(options)`  | `LogOptions`   | `Result<LogIterator>` |

LogOptionsでサポートするフィルタ:
- パス指定（複数可）
- 件数制限
- 日付範囲（since/until）
- first-parent
- 作者フィルタ

#### 2.2.5.4 Tree Diff

| 機能                       | 入力                    | 出力              |
| -------------------------- | ----------------------- | ----------------- |
| `diff_trees(old, new)`     | 2つのTree               | `Result<TreeDiff>` |
| `commit_diff(commit)`      | コミット                | `Result<TreeDiff>` |
| `diff_index_to_workdir()`  | -                       | `Result<TreeDiff>` |
| `diff_head_to_index()`     | -                       | `Result<TreeDiff>` |
| `diff_head_to_workdir()`   | -                       | `Result<TreeDiff>` |

#### 2.2.5.5 Commit OID・branches API

| 機能              | 入力 | 出力               |
| ----------------- | ---- | ------------------ |
| `Commit::oid()`   | -    | `&Oid`             |
| `Oid::short()`    | -    | `String`（7文字）  |
| `branches()`      | -    | `Result<Vec<Branch>>` |

#### 2.2.5.6 Git Config読み取り

| 機能               | 入力              | 出力             |
| ------------------ | ----------------- | ---------------- |
| `config()`         | -                 | `Result<Config>` |
| `Config::get(key)` | "section.key"形式 | `Option<&str>`   |

### Phase 3: 差分・マージ（将来）

| 機能                  | 入力                  | 出力         | 優先度 | 状態      |
| --------------------- | --------------------- | ------------ | ------ | --------- |
| `diff()`              | 2つのコミット or tree | 差分情報     | 中     | ✅ 完了   |
| merge（fast-forward） | ブランチ名            | `Result<()>` | 低     | 未着手    |
| merge（3-way）        | ブランチ名            | `Result<()>` | 低     | 未着手    |
| packfile読み取り      | -                     | -            | 中     | 未着手    |

### Phase 4: リモート操作（将来・別crate検討）

| 機能  | 備考                   |
| ----- | ---------------------- |
| fetch | HTTPSならTLS依存が発生 |
| push  | 同上                   |
| clone | 同上                   |

**注:** リモート操作は最小依存原則と競合するため、別crateまたはfeature flag化を検討。

---

## 3. 非機能要件

### 3.1 性能

| 項目                 | 要件                                                 |
| -------------------- | ---------------------------------------------------- |
| オブジェクト読み取り | 1000オブジェクト/秒以上                              |
| status取得           | 10,000ファイル規模で1秒以内                          |
| メモリ使用量         | 大規模リポジトリでもストリーミング処理で一定に抑える |

### 3.2 互換性

| 項目            | 要件                           |
| --------------- | ------------------------------ |
| Rust バージョン | Rust 1.70+ (Edition 2021)      |
| OS              | Linux, macOS, Windows          |
| Git バージョン  | Git 2.x で作成されたリポジトリ |
| index形式       | v2 必須、v3/v4 は将来対応      |

### 3.3 信頼性

| 項目               | ポリシー                                 |
| ------------------ | ---------------------------------------- |
| エラーハンドリング | `Result<T, Error>` を返す。panicは避ける |
| エラー型           | `thiserror` を使用せず自前のError enum   |
| 不正データ         | 破損したオブジェクトは明確なエラーで報告 |
| 部分的失敗         | 可能な限り処理を継続し、エラーを収集     |

### 3.4 セキュリティ

| 項目               | 対策                            |
| ------------------ | ------------------------------- |
| パストラバーサル   | `.git` 外へのアクセスを禁止     |
| シンボリックリンク | デフォルトで追跡しない          |
| 入力検証           | SHA-1形式、参照名の妥当性を検証 |
| メモリ安全性       | Rustの所有権システムに依存      |

---

## 4. 制約条件

### 4.1 使用言語・フレームワーク

| 項目         | 選定                                       |
| ------------ | ------------------------------------------ |
| 言語         | Rust (Edition 2021)                        |
| 依存ポリシー | Pure Rust crateのみ（Cバインディング禁止） |

### 4.2 依存crate

| crate       | 用途     | 備考                |
| ----------- | -------- | ------------------- |
| miniz_oxide | zlib解凍 | Pure Rust、依存なし |

**自前実装するもの:**
- SHA-1ハッシュ（RFC 3174準拠）
- Git indexパーサー

### 4.3 ライセンス

| 項目            | 選定                                    |
| --------------- | --------------------------------------- |
| 本プロジェクト  | MIT OR Apache-2.0（デュアルライセンス） |
| 依存crateの制約 | GPL系ライセンスは不可                   |

---

## 5. API設計案

```rust
// リポジトリ操作
let repo = Repository::discover(".")?;
let repo = Repository::open("/path/to/.git")?;

// コミット履歴
for commit in repo.log()?.take(10) {
    println!("{}: {}", commit.id.short(), commit.summary());
}

// ステータス
for entry in repo.status()? {
    println!("{:?} {}", entry.status, entry.path);
}

// Phase 2
repo.add("issues/ISS-001.md")?;
repo.commit("Add new issue")?;
```

---

## 6. プロジェクト管理ツールとの連携ポイント

| 機能       | 用途                          |
| ---------- | ----------------------------- |
| `status()` | 変更されたissueファイルの検出 |
| `log()`    | issue変更履歴の表示           |
| `commit()` | issue作成・更新のコミット     |
| `diff()`   | issue内容の変更差分表示       |

---

## 7. 開発順序（推奨）

1. ✅ SHA-1実装
2. ✅ オブジェクト読み取り（blob → tree → commit）
3. ✅ refs解決
4. ✅ log機能
5. ✅ index読み取り
6. ✅ status機能
7. --- MVP完了 (Phase 1) ---
8. ✅ index書き込み
9. ✅ add/commit機能
10. ✅ ブランチ操作
11. --- Phase 2 完了 ---
12. ✅ リモートブランチ・タグ一覧
13. ✅ ログフィルタリング（パス、件数、日付）
14. ✅ Tree Diff実装
15. ✅ Working Tree / Index Diff
16. ✅ Repository::init()
17. ✅ Commit::oid() / Oid::short()
18. ✅ Repository::branches()
19. ✅ Config読み取り
20. --- Phase 2.5 完了 (v0.3.7) ---

---

## 8. 参考資料

- [Git Internals - Git Objects](https://git-scm.com/book/en/v2/Git-Internals-Git-Objects)
- [gitoxide](https://github.com/Byron/gitoxide) - Pure Rust実装の参考
- [miniz_oxide](https://github.com/Frommi/miniz_oxide) - Pure Rust zlib実装
- [RFC 3174 - SHA-1](https://datatracker.ietf.org/doc/html/rfc3174)
- [Git index format](https://git-scm.com/docs/index-format)
