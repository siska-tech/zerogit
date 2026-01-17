# Issue #017: Repository オブジェクト取得メソッド実装

## Phase
Phase 1: Repository Layer

## 説明
commit/tree/blob/objectの取得メソッドを実装する。

## タスク
- [x] `resolve_short_oid()` を実装
- [x] `commit()` を実装
- [x] `tree()` を実装
- [x] `blob()` を実装
- [x] `object()` を実装
- [x] 統合テストを作成

## テストケース
- RP-010〜RP-013, RP-030〜RP-034（テスト仕様書参照）

## 受け入れ条件
- [x] 完全SHA/短縮SHAでオブジェクトを取得できる
- [x] 型不一致でエラーを返す
- [x] テストがパス

## 依存
- #012
- #016

## 実装詳細

### 追加されたメソッド (repository.rs)

1. **`resolve_short_oid(&self, short_oid: &str) -> Result<Oid>`**
   - 短縮OID（4文字以上）から完全OIDを解決
   - 40文字の場合はそのまま解析
   - 曖昧な場合（複数一致）は`Error::InvalidOid`を返す

2. **`commit(&self, oid_str: &str) -> Result<Commit>`**
   - 完全SHA/短縮SHAでコミットを取得
   - 型不一致時は`Error::TypeMismatch`を返す

3. **`tree(&self, oid_str: &str) -> Result<Tree>`**
   - 完全SHA/短縮SHAでツリーを取得
   - 型不一致時は`Error::TypeMismatch`を返す

4. **`blob(&self, oid_str: &str) -> Result<Blob>`**
   - 完全SHA/短縮SHAでblobを取得
   - 型不一致時は`Error::TypeMismatch`を返す

5. **`object(&self, oid_str: &str) -> Result<Object>`**
   - 完全SHA/短縮SHAでオブジェクトを取得
   - `Object::Blob`, `Object::Tree`, `Object::Commit`のいずれかを返す

### テスト結果
- 全177テストがパス
- テストカバレッジ: RP-010〜RP-013, RP-030〜RP-034を含む
