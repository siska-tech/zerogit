# Issue #001: プロジェクト初期化

## Phase
Phase 0: プロジェクトセットアップ

## 説明
Cargoプロジェクトの作成と基本構成のセットアップ。

## タスク
- [x] `cargo new zerogit --lib` でプロジェクト作成
- [x] `Cargo.toml` の設定（メタデータ、依存関係）
- [x] ディレクトリ構造の作成
- [x] `.gitignore` の作成
- [x] LICENSE-MIT, LICENSE-APACHE の作成
- [x] README.md の配置

## Cargo.toml
```toml
[package]
name = "zerogit"
version = "0.1.0"
edition = "2021"
rust-version = "1.70"
description = "A lightweight, pure Rust Git client library"
license = "MIT OR Apache-2.0"
repository = "https://github.com/siska-tech/zerogit"
keywords = ["git", "vcs", "pure-rust"]
categories = ["development-tools"]

[dependencies]
miniz_oxide = "0.8"

[dev-dependencies]
tempfile = "3"
```

## ディレクトリ構造
```
zerogit/
├── Cargo.toml
├── LICENSE-MIT
├── LICENSE-APACHE
├── README.md
├── src/
│   ├── lib.rs
│   ├── error.rs
│   ├── repository.rs
│   ├── objects/
│   │   └── mod.rs
│   ├── refs/
│   │   └── mod.rs
│   ├── index/
│   │   └── mod.rs
│   └── infra/
│       └── mod.rs
└── tests/
    ├── fixtures/
    │   └── create_fixtures.sh
    └── integration/
```

## 受け入れ条件
- [x] `cargo build` が成功する
- [x] `cargo test` が実行できる（テストは空でOK）
- [x] `cargo clippy` で警告なし

## 依存
なし

## ステータス
**完了** (2026-01-17)
