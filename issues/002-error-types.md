# Issue #002: エラー型の定義

## Phase
Phase 0: プロジェクトセットアップ

## 説明
プロジェクト全体で使用するエラー型を定義する。

## タスク
- [x] `src/error.rs` を作成
- [x] `Error` enum を定義
- [x] `Display`, `std::error::Error` トレイトを実装
- [x] `From<std::io::Error>` を実装
- [x] `Result<T>` 型エイリアスを定義

## 実装内容
```rust
// src/error.rs
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    NotARepository(std::path::PathBuf),
    ObjectNotFound(Oid),
    RefNotFound(String),
    PathNotFound(std::path::PathBuf),
    InvalidOid(String),
    InvalidRefName(String),
    InvalidObject { oid: Oid, reason: String },
    InvalidIndex { version: u32, reason: String },
    TypeMismatch { expected: &'static str, actual: &'static str },
    InvalidUtf8,
    DecompressionFailed,
    // Phase 2
    RefAlreadyExists(String),
    CannotDeleteCurrentBranch,
    EmptyCommit,
    DirtyWorkingTree,
    ConfigNotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;
```

## 受け入れ条件
- [x] すべてのエラーバリアントが定義されている
- [x] `Display` で人間可読なメッセージが出力される
- [x] テスト E-001〜E-004 がパス

## 依存
- #001

## ステータス
**完了** (2026-01-17)

## 備考
- `Oid`型は未定義のため、`ObjectNotFound`と`InvalidObject`では一時的に`String`型を使用
- Issue #007（Oid実装）完了後に`Oid`型に変更予定
