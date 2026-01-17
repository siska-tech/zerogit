# Issue #022: Index書き込み実装

## Phase
Phase 2: 書き込み操作

## 説明
Indexファイルの書き込み機能を実装する。

## タスク
- [x] `src/index/writer.rs` を作成
- [x] `Index::write()` を実装
- [x] `write_entry()` を実装
- [x] チェックサム計算を実装
- [x] ユニットテストを作成

## 受け入れ条件
- [x] Indexをファイルに書き込める
- [x] gitコマンドで読み取れる形式で出力される

## 依存
- #015
- #004

## 実装詳細

### 作成したファイル
- `src/index/writer.rs` - Index書き込み機能

### 主要な関数
- `write(index: &Index) -> Vec<u8>` - Indexをバイト列にシリアライズ
- `write_header()` - ヘッダー（DIRC + version + entry_count）を書き込み
- `write_entry()` - 各エントリを書き込み（メタデータ + OID + flags + path + padding）
- `file_mode_to_u32()` - FileModeをu32に変換
- `path_to_unix_bytes()` - パスをUnixスタイル（/区切り）に変換

### テストケース
- IW-001: 空のindex書き込み
- IW-002: 単一エントリの書き込み
- IW-003: 複数エントリの書き込み
- IW-004: ラウンドトリップテスト（write → parse）
- IW-005: チェックサム検証
- IW-006: FileMode変換
- IW-007: 実行可能ファイルモード
- IW-008: ステージ番号（マージコンフリクト）
- IW-009: Windowsパス変換
- IW-010: Version 3 index
- IW-011: パディングアライメント
