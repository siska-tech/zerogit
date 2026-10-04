# Issue #038: packed-refsの参照解決・列挙と既存書き込み操作の保護

## 基本情報

| 項目 | 内容 |
|---|---|
| Phase | 3: KazeNhanh向け読み取り・差分 |
| 優先度 | 必須・高 |
| 依存（ローカル計画ID） | なし |
| ステータス | 実装済み（ローカル・未コミット） |

## 背景・目的

RefStoreはloose refsのみ対応しており、packed参照のHEAD・ブランチ・タグを取得できない。

## タスク

- [x] loose優先、loose不存在時のみpacked-refsへフォールバックする
- [x] HEADの間接参照、heads・remotes・tagsの一覧統合、重複除去、順序の安定化を実装する
- [x] コメント・peeled行を解析し、注釈付きタグ本体のOIDとpeeled OIDを区別する
- [x] create_branchの存在確認を統一する。packed参照削除は初期段階では変更前に未対応エラーとし、loose削除後のpacked参照復活を防ぐ
- [x] resolveや一覧処理、tagsで破損・I/Oエラーを握りつぶさない

## 受け入れ条件

- [x] git pack-refs --all --prune前後でHEAD・各参照一覧とOIDが一致する
- [x] looseとpackedの重複、注釈付きタグ、参照循環、破損、unborn HEADを検証する
- [x] packed参照と同名の作成を拒否し、削除拒否時にloose/packedファイルが変化しない

## 主な対象

src/refs/resolver.rs、src/repository.rs、tests/refs_test.rs、tests/branch_test.rs

## 共通方針

Pure Rustと最小依存を維持する。API名は実装時に既存設計と整合させる。新規公開APIは文書化し、変更に対応するテストを追加する。初期リリース対象はSHA-1、loose/pack、loose refs/packed-refsとする。3-way merge・リモート通信・worktree・shallow/partial clone・alternates・reftableの対応拡張は別計画とする。

## 参照

- [実装計画一覧](zerogit-issues.md)
- [Git pack仕様](https://git-scm.com/docs/gitformat-pack)
- [Gitリポジトリ構造](https://git-scm.com/docs/gitrepository-layout)


## GitHub

- Issue: https://github.com/siska-tech/zerogit/issues/8
- 親計画: https://github.com/siska-tech/zerogit/issues/6

## 実装・検証記録（2026-10-04）

- SHA-1 packed-refsのパーサーを追加した。コメント、CRLF、peeled行を扱い、タグ本体のOIDを保持する。重複参照、孤立したpeeled行、不正OID・参照名をInvalidPackedRefs（行番号付き）として返す。
- RefStoreはloose参照の不存在時のみpackedへフォールバックする。破損・I/O失敗は伝播する。参照名をファイルパスとして使う前に検証する。
- heads/tags/remotesの一覧を統合・重複除去・安定順序化した。Repositoryの一覧取得は操作単位でpacked-refsを1回読み、注釈付きタグの読み取り・解析エラーも伝播する。
- ブランチ作成はpacked側の既存参照も確認する。packed側に同名参照がある削除は、looseとの重複時もPackedRefDeletionUnsupportedで変更前に停止する。checkoutも参照解決エラーを握りつぶさない。
- 新規結合テスト8件とパーサーテスト2件を追加した。git pack-refs --all --prune前後のHEAD・参照一覧・タグ情報の一致、packedブランチcheckout、loose優先、破損、循環、unborn HEAD、削除時のファイル不変を検証した。
- Windows / installed stableで全489テスト成功。既存Clippy診断との差分は0件。変更ファイルはrustfmt済み、git diff --check成功。全体fmt/Clippyの既存問題は残る。Rust 1.70と他OSはCIで確認する。
- packed-refsの変更・削除処理とPackfileオブジェクト読み取りは、このIssueの実装に含めない。GitHub Issueはレビュー・反映前のためopenを維持する。