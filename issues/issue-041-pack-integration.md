# Issue #041: Pack読み取りを全APIへ統合しrepack前後の互換性を検証

## 基本情報

| 項目 | 内容 |
|---|---|
| Phase | 3: KazeNhanh向け読み取り・差分 |
| 優先度 | 必須・高 |
| 依存（ローカル計画ID） | 037、038、040 |
| ステータス | 実装済み（PR #17） |

## 背景・目的

Packの個別パーサーを実際のRepository APIと履歴走査へ接続する。

## タスク

- [x] looseと複数packを横断し、read・存在確認・prefix検索で同一OIDを重複排除する
- [x] 別packやlooseのREF_DELTA基底を統一ストアで解決する
- [x] Repository内で索引を再利用し、キャッシュの容量と寿命を明確にする
- [x] 外部repack時は限定的な再読込か明示的エラーとし、無限再試行や黙った欠落を避ける
- [x] 未対応objectFormat（SHA-256等）を入口で明示する
- [x] Git CLIはfixture生成・期待値比較のみに使用し、製品の実行時依存にしない

## 受け入れ条件

- [x] repack前後でcommit・tree・blob・tag・log・パスフィルタ・TreeDiffの結果が一致する
- [x] looseのみ・packのみ・混在・複数pack・同一OID重複・prefix衝突を検証する
- [x] packed-refsとの組合せおよび既存status・書き込み操作の回帰を検証する

## 主な対象

src/objects/database.rs、src/objects/store.rs、src/repository.rs、src/log.rs、src/error.rs、tests/pack_integration_test.rs

## 実装メモ

- `ObjectStore`はArcで共有するハンドルで、`Repository`が1つ保持し、`log`系のイテレータと共有する。packディレクトリの走査とidx解析は初回利用時に行い、Repositoryの寿命の間は再利用する。delta復元キャッシュはpackごとに`PackLimits::delta_cache_size`（既定32MiB）。
- 読み取り順はloose → pack（名前順）。同一OIDは先に見つかった方を返し、前方一致検索は重複を除いて返す。
- 外部repack対策: read・existsで見つからない場合だけpackディレクトリを1回再走査し、変化があれば1回だけ再検索する。前方一致検索は、古いpack一覧で曖昧なprefixを一意と誤判定しないよう毎回再走査する（開いているpackは再利用）。開けないpackは読み飛ばさずエラーにする。`.idx`のない`.pack`は書き込み中とみなして無視する。multi-pack-indexは使わず、個々の`.idx`を読む。
- pack外のREF_DELTA基底はストア経由で他packやlooseから解決する。残り深度を引き継ぎ、pack間の移動は64回までとする（pack間の循環やスタック溢れを防ぐ）。
- `Repository::open`、`discover`、`init`で、`extensions.objectFormat`がsha1以外、`extensions.refStorage`がfiles以外、`repositoryformatversion`が0/1以外のリポジトリを`UnsupportedRepositoryFormat`として拒否する。
- テストとfixture生成では、Gitのバックグラウンド自動メンテナンス（`maintenance.auto`、`gc.auto`）を無効にしている。Git 2.55ではcommit後に増分repackが走り、テストが非決定的になるため。

## 共通方針

Pure Rustと最小依存を維持する。API名は実装時に既存設計と整合させる。新規公開APIは文書化し、変更に対応するテストを追加する。初期リリース対象はSHA-1、loose/pack、loose refs/packed-refsとする。3-way merge・リモート通信・worktree・shallow/partial clone・alternates・reftableの対応拡張は別計画とする。

## 参照

- [実装計画一覧](zerogit-issues.md)
- [Git pack仕様](https://git-scm.com/docs/gitformat-pack)
- [Gitリポジトリ構造](https://git-scm.com/docs/gitrepository-layout)


## GitHub

- Issue: https://github.com/siska-tech/zerogit/issues/11
- 親計画: https://github.com/siska-tech/zerogit/issues/6
- 依存Issue: https://github.com/siska-tech/zerogit/issues/7, https://github.com/siska-tech/zerogit/issues/8, https://github.com/siska-tech/zerogit/issues/10
