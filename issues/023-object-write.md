# Issue #023: オブジェクト書き込み実装

## Phase
Phase 2: 書き込み操作

## 説明
Looseオブジェクトの書き込み機能を実装する。

## タスク
- [x] `compress()` 関数を実装（infra/compression.rs）
- [x] `LooseObjectStore::write()` を実装
- [x] ユニットテストを作成

## 受け入れ条件
- [x] オブジェクトをファイルに書き込める
- [x] gitコマンドで読み取れる

## 依存
- #008
- #005

## 実装詳細

### 変更したファイル
- `src/infra/compression.rs` - `compress()` 関数を追加
- `src/infra/mod.rs` - `compress` と `write_file_atomic` をエクスポート
- `src/objects/store.rs` - `LooseObjectStore::write()` を追加

### 主要な関数

#### `compress(data: &[u8]) -> Vec<u8>`
- zlibフォーマットでデータを圧縮
- 圧縮レベル6（デフォルト、速度とサイズのバランスが良い）

#### `LooseObjectStore::write(object_type: ObjectType, content: &[u8]) -> Result<Oid>`
1. Gitヘッダーを作成: `<type> <size>\0<content>`
2. SHA-1ハッシュを計算
3. zlibで圧縮
4. `.git/objects/xx/yyy...` パスに書き込み
5. 冪等性: 既存のオブジェクトは再書き込みしない

### テストケース
#### compression
- C-005: 圧縮→展開のラウンドトリップ
- C-006: 空データの圧縮
- C-007: 大量データの圧縮
- C-008: 繰り返しデータの圧縮率

#### store
- S-011: write() でオブジェクトファイルを作成
- S-012: write() は冪等（同じ内容は同じOID）
- S-013: write() は異なるオブジェクトタイプに対応
- S-014: write() は正しいディレクトリ構造を作成
- S-015: write() は正しいハッシュを生成（既知のハッシュと比較）
- S-016: write() は大量データに対応
