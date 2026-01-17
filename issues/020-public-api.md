# Issue #020: 公開API整理とlib.rs実装

## Phase
Phase 1: Repository Layer

## 説明
公開APIを整理し、lib.rsでre-exportする。

## タスク
- [x] `src/lib.rs` を整理
- [x] 公開する型をre-export
- [x] ドキュメントコメントを追加
- [x] クレートレベルのドキュメントを追加

## 公開API
```rust
// src/lib.rs
pub use error::{Error, Result};
pub use repository::Repository;
pub use objects::{Object, Oid, Blob, Tree, TreeEntry, Commit, Signature, FileMode};
pub use refs::{Head, Branch};
pub use status::{StatusEntry, FileStatus};
pub use index::{Index, IndexEntry};
```

## 受け入れ条件
- [x] `use zerogit::*;` で必要な型がインポートできる
- [x] `cargo doc` でドキュメントが生成される
- [x] READMEのコード例がコンパイルできる

## 依存
- #016
- #017
- #018
- #019

## 完了
- 2026-01-17: 全タスク完了
