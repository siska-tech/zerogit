# Issue #015: Indexパース実装

## Phase
Phase 1: Index Layer

## 説明
.git/indexファイルのパース機能を実装する。

## タスク
- [x] `src/index/mod.rs` を作成
- [x] `src/index/reader.rs` を作成
- [x] `Index`, `IndexEntry` 構造体を定義
- [x] `parse()` を実装
- [x] `parse_header()` を実装
- [x] `parse_entry()` を実装
- [x] 各アクセサメソッドを実装
- [x] ユニットテストを作成

## テストケース
- I-001〜I-005（テスト仕様書参照）

## 受け入れ条件
- [x] v2形式のIndexをパースできる
- [x] 全エントリを正しく取得できる
- [x] 不正な形式でエラーを返す
- [x] テスト I-001〜I-005 がパス

## 依存
- #006
- #007

## 実装詳細

### 作成/変更したファイル
- `src/index/mod.rs` - Index, IndexEntry構造体とアクセサメソッド
- `src/index/reader.rs` - parse(), parse_header(), parse_entry()関数

### 実装した機能
- Git index v2/v3/v4 形式のパース
- エントリのメタデータ（ctime, mtime, dev, ino, uid, gid, size）の取得
- ファイルパス、モード、OIDの取得
- ステージ番号（コンフリクト状態）の取得
- パス検索（get()メソッド）

### テスト結果
- 18件のユニットテストが全てパス
- clippy警告なし（index関連）
