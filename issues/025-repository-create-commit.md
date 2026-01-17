# Issue #025: Repository::create_commit()実装

## Phase
Phase 2: 書き込み操作

## 説明
コミット作成機能を実装する。

## タスク
- [x] `build_tree_from_index()` を実装
- [x] `format_commit()` を実装
- [x] `update_head()` を実装
- [x] `Repository::create_commit()` を実装
- [x] 統合テストを作成

## テストケース
- W-004〜W-005（テスト仕様書参照）

## 受け入れ条件
- [x] コミットを作成できる
- [x] HEADが更新される
- [x] `git log` で表示される

## 依存
- #023
- #024

## 実装詳細

### 追加されたメソッド

1. **`Repository::create_commit(message, author_name, author_email)`**
   - インデックスからコミットを作成
   - ツリー構築 → コミットオブジェクト作成 → HEAD更新
   - 空のインデックスでは `Error::EmptyCommit` を返す

2. **`build_tree_from_index(idx)`** (private)
   - インデックスエントリからツリーオブジェクトを構築
   - 深いディレクトリから浅いディレクトリへと処理（ボトムアップ）
   - サブディレクトリは再帰的にツリーとして作成

3. **`build_tree_content(entries)`** (private)
   - ツリーオブジェクトのバイナリ形式を生成
   - フォーマット: `<mode> <name>\0<20-byte-sha1>`

4. **`format_commit(tree_oid, parent_oid, author, committer, message)`** (private)
   - コミットオブジェクトの内容を生成
   - 親コミットがある場合は parent 行を含める

5. **`update_head(new_oid)`** (private)
   - HEADを新しいコミットに更新
   - ブランチの場合はブランチ参照を更新
   - detached HEADの場合はHEADを直接更新

### テスト
- `test_create_commit_with_staged_files` (W-004)
- `test_create_commit_empty_index` (W-005)
- `test_create_commit_with_subdirectories`
- `test_create_commit_chain`
- `test_create_commit_updates_branch`
- `test_build_tree_content`
- `test_format_commit`
- `test_format_commit_no_parent`
