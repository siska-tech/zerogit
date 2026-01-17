# Issue #016: Repository基本実装

## Phase
Phase 1: Repository Layer

## 説明
Repositoryの基本機能（open/discover）を実装する。

## タスク
- [x] `src/repository.rs` を作成
- [x] `Repository` 構造体を定義
- [x] `validate_git_dir()` を実装
- [x] `open()` を実装
- [x] `discover()` を実装
- [x] `path()`, `git_dir()` を実装
- [x] 統合テストを作成

## テストケース
- RP-001〜RP-007（テスト仕様書参照）

## 受け入れ条件
- [x] 有効なリポジトリを開ける
- [x] 親ディレクトリを遡って.gitを発見できる
- [x] 無効なパスでエラーを返す
- [x] テスト RP-001〜RP-007 がパス

## 依存
- #008
- #013
- #015

## 実装詳細

### Repository構造体
```rust
pub struct Repository {
    work_dir: PathBuf,  // 作業ディレクトリ
    git_dir: PathBuf,   // .gitディレクトリ
}
```

### 主要メソッド
- `open(path)` - 指定パスでリポジトリを開く（.gitディレクトリでも可）
- `discover(path)` - 親ディレクトリを遡ってリポジトリを探す
- `path()` - 作業ディレクトリのパスを返す
- `git_dir()` - .gitディレクトリのパスを返す
- `validate_git_dir(git_dir)` - .gitディレクトリが有効か検証（HEAD, objects, refsの存在確認）

### テスト結果
- 単体テスト: 12件パス
- 統合テスト: 11件パス（RP-001〜RP-007 + 追加テスト）
