# Issue #037: オブジェクト読み取り経路をObjectStoreへ統一

## 基本情報

| 項目 | 内容 |
|---|---|
| Phase | 3: KazeNhanh向け読み取り・差分 |
| 優先度 | 必須・高 |
| 依存（ローカル計画ID） | なし |
| ステータス | 実装済み（ローカル・未コミット） |

## 背景・目的

Repository、LogIterator、statusがLooseObjectStoreに直接依存しており、RepositoryのみのPack対応では読み取り経路が残る。

## タスク

- [x] 内部ObjectStoreにread・存在確認・短縮OID検索を集約し、repository.rs・log.rs・status.rsを移行する
- [x] 既存公開APIの互換性を維持し、書き込みはloose形式を継続する
- [x] 不存在・破損・未対応形式・I/O失敗を区別し、破損を不存在として扱わない

## 受け入れ条件

- [x] 既存loose objectの取得・履歴・status・差分・書き込みの回帰テストが通る
- [x] 短縮OIDの一意・曖昧・不存在を区別できる

## 主な対象

src/objects/store.rs、src/repository.rs、src/log.rs、src/status.rs、src/error.rs

## 共通方針

Pure Rustと最小依存を維持する。API名は実装時に既存設計と整合させる。新規公開APIは文書化し、変更に対応するテストを追加する。初期リリース対象はSHA-1、loose/pack、loose refs/packed-refsとする。3-way merge・リモート通信・worktree・shallow/partial clone・alternates・reftableの対応拡張は別計画とする。

## 参照

- [実装計画一覧](zerogit-issues.md)
- [Git pack仕様](https://git-scm.com/docs/gitformat-pack)
- [Gitリポジトリ構造](https://git-scm.com/docs/gitrepository-layout)


## GitHub

- Issue: https://github.com/siska-tech/zerogit/issues/7
- 親計画: https://github.com/siska-tech/zerogit/issues/6

## 実装・検証記録（2026-10-04）

- 内部ObjectStoreを追加し、Repository・LogIterator・statusの読み取りと短縮OID検索を統一した。公開LooseObjectStoreとstatus補助関数の引数は維持し、内部経路に委譲する。
- 書き込みはloose形式を継続し、既存オブジェクトの破損を正常な書き込みとして扱わない。
- loose読み取りでヘッダの余分なフィールドと内容のOID不一致を検出する。短縮OID検索はディレクトリを候補から除外し、I/O失敗を伝播する。
- status・add_all・reset・checkoutのHEADコミット読み取りエラーを伝播し、初期ブランチと破損・欠落を区別する。create_commitではHEAD解決エラーを握りつぶさない。
- 不存在・破損・I/O失敗の区別はloose形式で検証済み。未対応pack/idx形式の具体的な判定とエラーは039〜041で追加する。
- Windows / installed stableでcargo test --offline --quietを実行し、単体・結合・doc testが成功（最終479件）。Git fixtureはユーザー設定とPowerShellの文字コードによる影響を除き、作業ファイルをLFにそろえて生成した。
- fmt全体チェックとClippy -D warningsは変更前HEADでも失敗する。Clippy診断を変更前と比較し、新規診断0件を確認した。新規ファイルと変更した主要ファイルはrustfmt済み。git diff --checkは成功。
- Rust 1.70と他OSの確認はCIで行う。GitHub Issueはレビュー・反映前のためopenを維持する。