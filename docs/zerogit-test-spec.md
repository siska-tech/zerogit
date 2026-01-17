# zerogit テスト仕様書

## 1. テスト方針

### 1.1 テスト種類

| 種類                 | 対象               | 目的                                 |
| -------------------- | ------------------ | ------------------------------------ |
| 単体テスト           | 各モジュール・関数 | 個々の機能の正確性を検証             |
| 結合テスト           | モジュール間連携   | コンポーネント間のデータフローを検証 |
| 統合テスト           | Repository API     | エンドツーエンドの動作を検証         |
| プロパティテスト     | パーサー           | ランダム入力での堅牢性を検証         |
| パフォーマンステスト | 重要機能           | 性能要件の達成を検証                 |

### 1.2 テストカバレッジ目標

| カテゴリ       | 目標    | 備考                     |
| -------------- | ------- | ------------------------ |
| 行カバレッジ   | 80%以上 | `cargo-tarpaulin` で計測 |
| 分岐カバレッジ | 70%以上 | エラーパスを含む         |
| 公開API        | 100%    | すべての公開関数にテスト |

### 1.3 テストフレームワーク

| ツール             | 用途                   |
| ------------------ | ---------------------- |
| Rust標準 `#[test]` | 単体テスト・結合テスト |
| `cargo test`       | テスト実行             |
| `cargo-tarpaulin`  | カバレッジ計測         |
| `criterion`        | ベンチマーク（将来）   |

### 1.4 テスト命名規則

```rust
#[test]
fn <対象>_<条件>_<期待結果>() {
    // 例: oid_from_hex_valid_40chars_returns_ok
    // 例: repository_open_invalid_path_returns_not_a_repository_error
}
```

---

## 2. テスト環境

### 2.1 実行環境

| 項目          | バージョン/条件                             |
| ------------- | ------------------------------------------- |
| Rust          | 1.70.0 以上（MSRV）                         |
| OS (CI)       | Ubuntu latest, macOS latest, Windows latest |
| OS (ローカル) | 開発者の環境                                |

### 2.2 CI設定

```yaml
# .github/workflows/test.yml
name: Test
on: [push, pull_request]
jobs:
  test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
        rust: [stable, 1.70.0]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: ${{ matrix.rust }}
      - run: cargo test --all-features
```

### 2.3 テストフィクスチャ

テスト用Gitリポジトリを `tests/fixtures/` に配置。

```
tests/
├── fixtures/
│   ├── simple/           # 基本的なリポジトリ
│   │   └── .git/
│   ├── empty/            # 空のリポジトリ（コミットなし）
│   │   └── .git/
│   ├── branches/         # 複数ブランチ
│   │   └── .git/
│   ├── merge/            # マージコミットあり
│   │   └── .git/
│   ├── large/            # 多数のファイル・コミット
│   │   └── .git/
│   └── corrupted/        # 破損したオブジェクト
│       └── .git/
└── integration/
    └── *.rs
```

### 2.4 フィクスチャ生成スクリプト

```bash
#!/bin/bash
# tests/fixtures/create_fixtures.sh

# simple: 基本リポジトリ
mkdir -p simple && cd simple
git init
echo "Hello" > README.md
git add README.md
git commit -m "Initial commit"
echo "World" >> README.md
git add README.md
git commit -m "Second commit"
cd ..

# empty: 空リポジトリ
mkdir -p empty && cd empty
git init
cd ..

# branches: 複数ブランチ
mkdir -p branches && cd branches
git init
echo "main" > file.txt
git add file.txt
git commit -m "Main commit"
git checkout -b feature
echo "feature" > feature.txt
git add feature.txt
git commit -m "Feature commit"
git checkout main
cd ..
```

---

## 3. テストケース

### 3.1 Infrastructure Layer

#### 3.1.1 SHA-1 ハッシュ（`infra::hash`）

| ID    | テスト項目           | 条件            | 期待結果                                   |
| ----- | -------------------- | --------------- | ------------------------------------------ |
| H-001 | 空データのハッシュ   | 空バイト列      | RFC 3174準拠の値                           |
| H-002 | 既知データのハッシュ | "hello world"   | `2aae6c35c94fcfb415dbe95f408b9ce91ee846ed` |
| H-003 | バイナリデータ       | NULを含むデータ | 正しいハッシュ値                           |
| H-004 | 大きなデータ         | 1MB以上         | 正しいハッシュ値                           |
| H-005 | Gitオブジェクト形式  | "blob 5\0hello" | Git互換のハッシュ                          |

```rust
#[test]
fn sha1_hello_world_returns_known_hash() {
    let hash = sha1(b"hello world");
    assert_eq!(
        hash.to_hex(),
        "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed"
    );
}

#[test]
fn sha1_empty_returns_known_hash() {
    let hash = sha1(b"");
    assert_eq!(
        hash.to_hex(),
        "da39a3ee5e6b4b0d3255bfef95601890afd80709"
    );
}

#[test]
fn sha1_git_blob_format() {
    // "hello" のGit blob形式ハッシュ
    let content = b"hello";
    let header = format!("blob {}\0", content.len());
    let mut data = header.into_bytes();
    data.extend_from_slice(content);
    let hash = sha1(&data);
    // git hash-object -t blob で確認した値
    assert_eq!(hash.to_hex(), "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0");
}
```

#### 3.1.2 zlib解凍（`infra::compression`）

| ID    | テスト項目     | 条件               | 期待結果                     |
| ----- | -------------- | ------------------ | ---------------------------- |
| C-001 | 正常解凍       | 有効なzlibデータ   | 元データを復元               |
| C-002 | 破損データ     | 不正なヘッダー     | `Error::DecompressionFailed` |
| C-003 | 空データ       | 空バイト列         | `Error::DecompressionFailed` |
| C-004 | 切り詰めデータ | 途中で切れたデータ | `Error::DecompressionFailed` |

```rust
#[test]
fn decompress_valid_zlib_returns_original() {
    let original = b"Hello, World!";
    let compressed = compress(original); // テスト用ヘルパー
    let result = decompress(&compressed).unwrap();
    assert_eq!(result, original);
}

#[test]
fn decompress_invalid_data_returns_error() {
    let invalid = b"not zlib data";
    let result = decompress(invalid);
    assert!(matches!(result, Err(Error::DecompressionFailed)));
}
```

---

### 3.2 Oid

| ID    | テスト項目       | 条件              | 期待結果                    |
| ----- | ---------------- | ----------------- | --------------------------- |
| O-001 | 有効な16進数     | 40文字の16進数    | `Ok(Oid)`                   |
| O-002 | 大文字16進数     | 大文字を含む      | `Ok(Oid)`（小文字に正規化） |
| O-003 | 短すぎる         | 39文字以下        | `Error::InvalidOid`         |
| O-004 | 長すぎる         | 41文字以上        | `Error::InvalidOid`         |
| O-005 | 不正な文字       | 16進数以外を含む  | `Error::InvalidOid`         |
| O-006 | to_hex往復       | from_hex → to_hex | 元の文字列と一致            |
| O-007 | short形式        | 任意のOid         | 7文字の文字列               |
| O-008 | バイト配列変換   | 20バイト          | `Ok(Oid)`                   |
| O-009 | バイト配列不正長 | 19または21バイト  | `Error::InvalidOid`         |

```rust
#[test]
fn oid_from_hex_valid_40chars_returns_ok() {
    let hex = "abc1234567890abcdef1234567890abcdef12345";
    let oid = Oid::from_hex(hex).unwrap();
    assert_eq!(oid.to_hex(), hex);
}

#[test]
fn oid_from_hex_uppercase_normalizes_to_lowercase() {
    let hex = "ABC1234567890ABCDEF1234567890ABCDEF12345";
    let oid = Oid::from_hex(hex).unwrap();
    assert_eq!(oid.to_hex(), hex.to_lowercase());
}

#[test]
fn oid_from_hex_short_returns_error() {
    let hex = "abc123";
    let result = Oid::from_hex(hex);
    assert!(matches!(result, Err(Error::InvalidOid(_))));
}

#[test]
fn oid_from_hex_invalid_char_returns_error() {
    let hex = "xyz1234567890abcdef1234567890abcdef12345";
    let result = Oid::from_hex(hex);
    assert!(matches!(result, Err(Error::InvalidOid(_))));
}

#[test]
fn oid_short_returns_7_chars() {
    let hex = "abc1234567890abcdef1234567890abcdef12345";
    let oid = Oid::from_hex(hex).unwrap();
    assert_eq!(oid.short(), "abc1234");
    assert_eq!(oid.short().len(), 7);
}
```

---

### 3.3 Objects

#### 3.3.1 Blobパース

| ID    | テスト項目             | 条件                        | 期待結果                    |
| ----- | ---------------------- | --------------------------- | --------------------------- |
| B-001 | 正常パース             | "blob 5\0hello"             | `Blob { content: "hello" }` |
| B-002 | 空Blob                 | "blob 0\0"                  | `Blob { content: "" }`      |
| B-003 | バイナリ               | NULを含む                   | 正しくパース                |
| B-004 | 不正ヘッダー           | "blob x\0..."               | `Error::InvalidObject`      |
| B-005 | サイズ不一致           | "blob 10\0hello"（5バイト） | `Error::InvalidObject`      |
| B-006 | content_str（UTF-8）   | 有効なUTF-8                 | `Ok(&str)`                  |
| B-007 | content_str（非UTF-8） | 無効なUTF-8                 | `Error::InvalidUtf8`        |
| B-008 | is_binary              | NULを含む                   | `true`                      |
| B-009 | is_binary              | テキストのみ                | `false`                     |

```rust
#[test]
fn blob_parse_valid_returns_content() {
    let raw = b"blob 5\0hello";
    let blob = Blob::parse(raw).unwrap();
    assert_eq!(blob.content(), b"hello");
    assert_eq!(blob.size(), 5);
}

#[test]
fn blob_parse_empty_returns_empty_content() {
    let raw = b"blob 0\0";
    let blob = Blob::parse(raw).unwrap();
    assert_eq!(blob.content(), b"");
    assert_eq!(blob.size(), 0);
}

#[test]
fn blob_content_str_valid_utf8_returns_ok() {
    let raw = b"blob 5\0hello";
    let blob = Blob::parse(raw).unwrap();
    assert_eq!(blob.content_str().unwrap(), "hello");
}

#[test]
fn blob_is_binary_with_nul_returns_true() {
    let raw = b"blob 6\0hel\0lo";
    let blob = Blob::parse(raw).unwrap();
    assert!(blob.is_binary());
}
```

#### 3.3.2 Treeパース

| ID    | テスト項目       | 条件           | 期待結果               |
| ----- | ---------------- | -------------- | ---------------------- |
| T-001 | 単一エントリ     | 1ファイル      | 1エントリのTree        |
| T-002 | 複数エントリ     | 複数ファイル   | ソート順で格納         |
| T-003 | ディレクトリ含む | blob + tree    | 両方パース             |
| T-004 | 空Tree           | エントリなし   | 空のentries            |
| T-005 | get検索          | 存在する名前   | `Some(&TreeEntry)`     |
| T-006 | get検索（不在）  | 存在しない名前 | `None`                 |
| T-007 | モード判定       | 100644         | `FileMode::Regular`    |
| T-008 | モード判定       | 100755         | `FileMode::Executable` |
| T-009 | モード判定       | 40000          | `FileMode::Tree`       |

```rust
#[test]
fn tree_parse_single_entry() {
    // "100644 file.txt\0<20-byte-sha>"
    let mut raw = b"tree 32\0100644 file.txt\0".to_vec();
    raw.extend_from_slice(&[0u8; 20]); // ダミーSHA
    
    let tree = Tree::parse(&raw).unwrap();
    assert_eq!(tree.entries().len(), 1);
    assert_eq!(tree.entries()[0].name(), "file.txt");
    assert_eq!(tree.entries()[0].mode(), FileMode::Regular);
}

#[test]
fn tree_get_existing_returns_some() {
    // フィクスチャから読み込み
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    let commit = repo.log().unwrap().next().unwrap().unwrap();
    let tree = repo.tree(&commit.tree().to_hex()).unwrap();
    
    assert!(tree.get("README.md").is_some());
}

#[test]
fn tree_get_nonexistent_returns_none() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    let commit = repo.log().unwrap().next().unwrap().unwrap();
    let tree = repo.tree(&commit.tree().to_hex()).unwrap();
    
    assert!(tree.get("nonexistent.txt").is_none());
}
```

#### 3.3.3 Commitパース

| ID     | テスト項目     | 条件             | 期待結果               |
| ------ | -------------- | ---------------- | ---------------------- |
| CM-001 | 正常パース     | 完全なコミット   | すべてのフィールド取得 |
| CM-002 | 初期コミット   | 親なし           | `parents.len() == 0`   |
| CM-003 | 通常コミット   | 親1つ            | `parents.len() == 1`   |
| CM-004 | マージコミット | 親2つ            | `parents.len() == 2`   |
| CM-005 | summary        | 複数行メッセージ | 1行目のみ              |
| CM-006 | summary        | 1行メッセージ    | そのまま               |
| CM-007 | author時刻     | タイムスタンプ   | 正しいUnix時刻         |
| CM-008 | オフセット     | +0900            | 540                    |
| CM-009 | オフセット     | -0500            | -300                   |

```rust
#[test]
fn commit_parse_extracts_all_fields() {
    let raw = b"commit 200\0\
tree abc1234567890abcdef1234567890abcdef12345
parent def1234567890abcdef1234567890abcdef12345
author John Doe <john@example.com> 1700000000 +0900
committer Jane Doe <jane@example.com> 1700000001 +0900

Initial commit

Body text here.
";
    let commit = Commit::parse(raw).unwrap();
    
    assert_eq!(commit.tree().to_hex(), "abc1234567890abcdef1234567890abcdef12345");
    assert_eq!(commit.parents().len(), 1);
    assert_eq!(commit.author().name(), "John Doe");
    assert_eq!(commit.author().email(), "john@example.com");
    assert_eq!(commit.author().time(), 1700000000);
    assert_eq!(commit.author().offset(), 540);
    assert_eq!(commit.summary(), "Initial commit");
    assert!(commit.message().contains("Body text here."));
}

#[test]
fn commit_initial_has_no_parents() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    // 最後のコミット（初期コミット）まで辿る
    let commits: Vec<_> = repo.log().unwrap().collect();
    let initial = commits.last().unwrap().as_ref().unwrap();
    
    assert!(initial.parents().is_empty());
    assert!(initial.parent().is_none());
}

#[test]
fn commit_summary_returns_first_line() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    let commit = repo.log().unwrap().next().unwrap().unwrap();
    
    // summaryは改行を含まない
    assert!(!commit.summary().contains('\n'));
}
```

---

### 3.4 Refs

| ID    | テスト項目               | 条件                  | 期待結果             |
| ----- | ------------------------ | --------------------- | -------------------- |
| R-001 | HEAD読み取り（ブランチ） | refs/heads/mainを指す | `Head::Branch`       |
| R-002 | HEAD読み取り（detached） | 直接SHA               | `Head::Detached`     |
| R-003 | ブランチ一覧             | 複数ブランチ          | すべて取得           |
| R-004 | symbolic-ref解決         | HEAD → main → SHA     | 最終SHA              |
| R-005 | 存在しないref            | 不正な参照名          | `Error::RefNotFound` |

```rust
#[test]
fn head_on_branch_returns_branch() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    let head = repo.head().unwrap();
    
    assert!(!head.is_detached());
    assert!(head.branch().is_some());
}

#[test]
fn branches_returns_all_local_branches() {
    let repo = Repository::open("tests/fixtures/branches").unwrap();
    let branches = repo.branches().unwrap();
    
    let names: Vec<_> = branches.iter().map(|b| b.name()).collect();
    assert!(names.contains(&"main"));
    assert!(names.contains(&"feature"));
}
```

---

### 3.5 Index

| ID    | テスト項目             | 条件           | 期待結果              |
| ----- | ---------------------- | -------------- | --------------------- |
| I-001 | v2インデックス読み取り | v2形式         | 正しくパース          |
| I-002 | エントリ取得           | 存在するパス   | `Some(&IndexEntry)`   |
| I-003 | エントリ取得（不在）   | 存在しないパス | `None`                |
| I-004 | 不正マジック           | DIRCでない     | `Error::InvalidIndex` |
| I-005 | 未対応バージョン       | v5以上         | `Error::InvalidIndex` |

```rust
#[test]
fn index_read_returns_entries() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    let index = repo.index().unwrap();
    
    assert!(index.len() > 0);
    assert!(index.get(Path::new("README.md")).is_some());
}

#[test]
fn index_version_is_supported() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    let index = repo.index().unwrap();
    
    assert!(index.version() >= 2 && index.version() <= 4);
}
```

---

### 3.6 Repository API

#### 3.6.1 open / discover

| ID     | テスト項目       | 条件                 | 期待結果                |
| ------ | ---------------- | -------------------- | ----------------------- |
| RP-001 | open正常         | 有効なリポジトリ     | `Ok(Repository)`        |
| RP-002 | open（.gitパス） | .gitディレクトリ指定 | `Ok(Repository)`        |
| RP-003 | open不正         | .gitがない           | `Error::NotARepository` |
| RP-004 | discover正常     | サブディレクトリから | `Ok(Repository)`        |
| RP-005 | discover不正     | ルートまでなし       | `Error::NotARepository` |
| RP-006 | path取得         | 正常リポジトリ       | リポジトリルート        |
| RP-007 | git_dir取得      | 正常リポジトリ       | .gitパス                |

```rust
#[test]
fn repository_open_valid_returns_ok() {
    let repo = Repository::open("tests/fixtures/simple");
    assert!(repo.is_ok());
}

#[test]
fn repository_open_invalid_returns_error() {
    let repo = Repository::open("/tmp/nonexistent");
    assert!(matches!(repo, Err(Error::NotARepository(_))));
}

#[test]
fn repository_discover_from_subdir_finds_root() {
    // tests/fixtures/simple/subdir から親の.gitを発見
    std::fs::create_dir_all("tests/fixtures/simple/subdir").ok();
    let repo = Repository::discover("tests/fixtures/simple/subdir").unwrap();
    
    assert!(repo.path().ends_with("simple"));
}

#[test]
fn repository_git_dir_returns_dot_git_path() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    assert!(repo.git_dir().ends_with(".git"));
}
```

#### 3.6.2 commit / log

| ID     | テスト項目       | 条件           | 期待結果                |
| ------ | ---------------- | -------------- | ----------------------- |
| RP-010 | commit完全SHA    | 40文字         | `Ok(Commit)`            |
| RP-011 | commit短縮SHA    | 7文字以上      | `Ok(Commit)`            |
| RP-012 | commit短すぎ     | 3文字以下      | `Error::InvalidOid`     |
| RP-013 | commit存在しない | 不正なSHA      | `Error::ObjectNotFound` |
| RP-014 | log取得          | 正常リポジトリ | `Ok(LogIterator)`       |
| RP-015 | log走査          | 複数コミット   | 新しい順                |
| RP-016 | log_from         | 途中から       | 指定コミット以降        |

```rust
#[test]
fn repository_commit_full_sha_returns_ok() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    let head = repo.head().unwrap();
    let full_sha = head.oid().to_hex();
    
    let commit = repo.commit(&full_sha);
    assert!(commit.is_ok());
}

#[test]
fn repository_commit_short_sha_returns_ok() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    let head = repo.head().unwrap();
    let short_sha = head.oid().short();
    
    let commit = repo.commit(&short_sha);
    assert!(commit.is_ok());
}

#[test]
fn repository_log_returns_commits_newest_first() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    let commits: Vec<_> = repo.log().unwrap()
        .filter_map(Result::ok)
        .collect();
    
    // 時刻が降順であることを確認
    for window in commits.windows(2) {
        assert!(window[0].author().time() >= window[1].author().time());
    }
}
```

#### 3.6.3 status

| ID     | テスト項目          | 条件         | 期待結果                |
| ------ | ------------------- | ------------ | ----------------------- |
| RP-020 | status（クリーン）  | 変更なし     | 空のVec                 |
| RP-021 | status（untracked） | 新規ファイル | `FileStatus::Untracked` |
| RP-022 | status（modified）  | 変更あり     | `FileStatus::Modified`  |
| RP-023 | status（deleted）   | 削除         | `FileStatus::Deleted`   |
| RP-024 | status（staged）    | add済み      | `FileStatus::Added`     |

```rust
#[test]
fn repository_status_clean_returns_empty() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    // フィクスチャはクリーン状態
    let status = repo.status().unwrap();
    
    // untrackedを除外してチェック
    let changes: Vec<_> = status.iter()
        .filter(|e| e.status() != FileStatus::Untracked)
        .collect();
    assert!(changes.is_empty());
}

#[test]
fn repository_status_new_file_is_untracked() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    
    // 一時ファイル作成
    let test_file = repo.path().join("test_untracked.txt");
    std::fs::write(&test_file, "test").unwrap();
    
    let status = repo.status().unwrap();
    let entry = status.iter()
        .find(|e| e.path().ends_with("test_untracked.txt"));
    
    assert!(entry.is_some());
    assert_eq!(entry.unwrap().status(), FileStatus::Untracked);
    
    // クリーンアップ
    std::fs::remove_file(test_file).ok();
}
```

#### 3.6.4 object / tree / blob

| ID     | テスト項目       | 条件              | 期待結果              |
| ------ | ---------------- | ----------------- | --------------------- |
| RP-030 | object（blob）   | BlobのSHA         | `Object::Blob`        |
| RP-031 | object（tree）   | TreeのSHA         | `Object::Tree`        |
| RP-032 | object（commit） | CommitのSHA       | `Object::Commit`      |
| RP-033 | tree型不一致     | BlobのSHAでtree() | `Error::TypeMismatch` |
| RP-034 | blob型不一致     | TreeのSHAでblob() | `Error::TypeMismatch` |

```rust
#[test]
fn repository_object_returns_correct_type() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    let head = repo.head().unwrap();
    
    let obj = repo.object(&head.oid().to_hex()).unwrap();
    assert!(matches!(obj, Object::Commit(_)));
}

#[test]
fn repository_tree_with_blob_sha_returns_type_mismatch() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    
    // README.mdのblobを取得
    let commit = repo.log().unwrap().next().unwrap().unwrap();
    let tree = repo.tree(&commit.tree().to_hex()).unwrap();
    let blob_entry = tree.get("README.md").unwrap();
    
    // blobのSHAでtree()を呼ぶ
    let result = repo.tree(&blob_entry.oid().to_hex());
    assert!(matches!(result, Err(Error::TypeMismatch { .. })));
}
```

---

### 3.7 エラーハンドリング

| ID    | テスト項目      | 条件           | 期待結果              |
| ----- | --------------- | -------------- | --------------------- |
| E-001 | Display実装     | すべてのエラー | 人間可読なメッセージ  |
| E-002 | Error実装       | すべてのエラー | std::error::Error互換 |
| E-003 | From<io::Error> | I/Oエラー      | `Error::Io`に変換     |
| E-004 | エラーチェーン  | 元エラー保持   | source()で取得可      |

```rust
#[test]
fn error_display_is_readable() {
    let err = Error::ObjectNotFound(Oid::from_hex(
        "abc1234567890abcdef1234567890abcdef12345"
    ).unwrap());
    
    let msg = format!("{}", err);
    assert!(msg.contains("abc1234"));
    assert!(msg.to_lowercase().contains("not found"));
}

#[test]
fn error_from_io_error_converts() {
    let io_err = std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "file not found"
    );
    let err: Error = io_err.into();
    
    assert!(matches!(err, Error::Io(_)));
}
```

---

### 3.8 境界値・エッジケース

| ID     | テスト項目         | 条件             | 期待結果                     |
| ------ | ------------------ | ---------------- | ---------------------------- |
| ED-001 | 空リポジトリ       | コミットなし     | `Error::RefNotFound`（HEAD） |
| ED-002 | 非常に長いパス     | 260文字以上      | 正常動作（Windows制限考慮）  |
| ED-003 | 非ASCII文字        | 日本語ファイル名 | 正常動作                     |
| ED-004 | シンボリックリンク | リンクファイル   | リンク自体を追跡             |
| ED-005 | 大きなファイル     | 100MB blob       | メモリ効率的に処理           |
| ED-006 | 深いコミット履歴   | 10000コミット    | タイムアウトなし             |

```rust
#[test]
fn empty_repository_head_returns_ref_not_found() {
    let repo = Repository::open("tests/fixtures/empty").unwrap();
    let result = repo.head();
    
    assert!(matches!(result, Err(Error::RefNotFound(_))));
}

#[test]
fn non_ascii_filename_works() {
    let repo = Repository::open("tests/fixtures/simple").unwrap();
    
    // 日本語ファイル作成
    let test_file = repo.path().join("日本語.txt");
    std::fs::write(&test_file, "テスト").unwrap();
    
    let status = repo.status().unwrap();
    let found = status.iter()
        .any(|e| e.path().to_string_lossy().contains("日本語"));
    
    assert!(found);
    
    std::fs::remove_file(test_file).ok();
}
```

---

### 3.9 パフォーマンステスト

| ID    | テスト項目           | 条件             | 期待結果  |
| ----- | -------------------- | ---------------- | --------- |
| P-001 | オブジェクト読み取り | 1000オブジェクト | 1秒以内   |
| P-002 | status               | 10000ファイル    | 1秒以内   |
| P-003 | log走査              | 1000コミット     | 500ms以内 |

```rust
#[test]
#[ignore] // CI環境では時間がかかるため
fn performance_log_1000_commits() {
    let repo = Repository::open("tests/fixtures/large").unwrap();
    
    let start = std::time::Instant::now();
    let count = repo.log().unwrap().take(1000).count();
    let elapsed = start.elapsed();
    
    assert!(count >= 1000);
    assert!(elapsed < std::time::Duration::from_millis(500));
}

#[test]
#[ignore]
fn performance_status_large_worktree() {
    let repo = Repository::open("tests/fixtures/large").unwrap();
    
    let start = std::time::Instant::now();
    let _ = repo.status().unwrap();
    let elapsed = start.elapsed();
    
    assert!(elapsed < std::time::Duration::from_secs(1));
}
```

---

## 4. Phase 2 テストケース（概要）

### 4.1 書き込み操作

| ID    | テスト項目    | 条件             | 期待結果                           |
| ----- | ------------- | ---------------- | ---------------------------------- |
| W-001 | add           | 新規ファイル     | インデックスに追加                 |
| W-002 | add           | 変更ファイル     | インデックス更新                   |
| W-003 | reset         | ステージ済み     | インデックスから除外               |
| W-004 | create_commit | ステージあり     | コミット作成、Oid返却              |
| W-005 | create_commit | ステージなし     | `Error::EmptyCommit`               |
| W-006 | create_branch | 有効な名前       | ブランチ作成                       |
| W-007 | create_branch | 重複名           | `Error::RefAlreadyExists`          |
| W-008 | delete_branch | 現在以外         | ブランチ削除                       |
| W-009 | delete_branch | 現在のブランチ   | `Error::CannotDeleteCurrentBranch` |
| W-010 | checkout      | 存在するブランチ | HEAD更新                           |

---

## 5. テスト実行手順

### 5.1 ローカル実行

```bash
# 全テスト実行
cargo test

# 特定モジュール
cargo test objects::

# 特定テスト
cargo test repository_open_valid

# 無視されたテスト含む
cargo test -- --ignored

# 出力表示
cargo test -- --nocapture
```

### 5.2 カバレッジ計測

```bash
# tarpaulinインストール
cargo install cargo-tarpaulin

# カバレッジ計測
cargo tarpaulin --out Html

# 結果確認
open tarpaulin-report.html
```

### 5.3 フィクスチャ準備

```bash
# フィクスチャ生成
cd tests/fixtures
./create_fixtures.sh

# フィクスチャ検証
git -C simple log --oneline
git -C branches branch -a
```

---

## 6. 継続的インテグレーション

### 6.1 GitHub Actions ワークフロー

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  test:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
        rust: [stable, 1.70.0]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: ${{ matrix.rust }}
      - name: Setup fixtures
        run: |
          cd tests/fixtures
          bash create_fixtures.sh
      - name: Run tests
        run: cargo test --all-features

  coverage:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Install tarpaulin
        run: cargo install cargo-tarpaulin
      - name: Setup fixtures
        run: |
          cd tests/fixtures
          bash create_fixtures.sh
      - name: Generate coverage
        run: cargo tarpaulin --out Xml
      - name: Upload coverage
        uses: codecov/codecov-action@v3
```

### 6.2 マージ条件

- [ ] 全テストがパス
- [ ] カバレッジ80%以上
- [ ] Clippy警告なし
- [ ] rustfmtフォーマット済み
