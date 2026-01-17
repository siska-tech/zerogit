# Issue #018: LogIterator実装

## Phase
Phase 1: Repository Layer

## 説明
コミット履歴を遅延取得するイテレータを実装する。

## タスク
- [x] `src/log.rs` を作成
- [x] `LogIterator` 構造体を定義
- [x] `PendingCommit` を定義（優先度キュー用）
- [x] `Iterator` トレイトを実装
- [x] `Repository::log()`, `log_from()` を実装
- [x] 統合テストを作成

## テストケース
- RP-014〜RP-016（テスト仕様書参照）

## 受け入れ条件
- [x] HEADからコミット履歴を取得できる
- [x] 時刻降順（新しい順）で取得される
- [x] マージコミットを正しく辿る
- [x] テスト RP-014〜RP-016 がパス

## 依存
- #014
- #017

## 実装詳細

### 追加されたファイル

**src/log.rs**
- `PendingCommit` - 優先度キュー用の内部構造体（タイムスタンプで比較）
- `LogIterator` - コミット履歴を遅延取得するイテレータ
  - `BinaryHeap` を使用して時刻降順でコミットを取得
  - `HashSet` で訪問済みコミットを追跡（重複防止）
  - マージコミットの全親を正しく辿る

### 追加されたメソッド (repository.rs)

1. **`head(&self) -> Result<Head>`**
   - 現在のHEADの状態を取得
   - ブランチ参照/detached HEAD の両方をサポート

2. **`log(&self) -> Result<LogIterator>`**
   - HEADからコミット履歴を取得

3. **`log_from(&self, start_oid: Oid) -> Result<LogIterator>`**
   - 指定したコミットから履歴を取得

### アルゴリズム
- 優先度キュー（最大ヒープ）を使用
- タイムスタンプが大きい（新しい）コミットを先に処理
- マージコミットでは全ての親をキューに追加
- 訪問済みコミットはスキップ（ダイヤモンド型のマージでも重複なし）

### テスト結果
- 全188テストがパス
- テストカバレッジ: RP-014〜RP-016を含む
