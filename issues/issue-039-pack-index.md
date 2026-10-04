# Issue #039: Pack index v2の解析とOID検索

## 基本情報

| 項目 | 内容 |
|---|---|
| Phase | 3: KazeNhanh向け読み取り・差分 |
| 優先度 | 必須・高 |
| 依存（ローカル計画ID） | 037 |
| ステータス | 実装済み（ローカル・未コミット） |

## 背景・目的

Pack内のオブジェクトをOIDから検索できる索引を追加する。

## タスク

- [x] SHA-1向けidx v2のmagic・version・fanout・OIDテーブル・CRC・offset・trailerを解析する
- [x] 64bit offset、完全OID検索、短縮OID検索を実装する
- [x] 件数・境界・ソート順・offset参照・checksumを検証し、切り詰めや整数overflowを拒否する
- [x] idx v1および未対応バージョンを明示的な未対応エラーにする

## 受け入れ条件

- [x] Git生成fixtureでOIDとoffsetが期待値に一致する
- [x] 大きなoffsetは小さな合成fixtureでも検証できる
- [x] 不正fanout、破損checksum、切り詰め、曖昧prefixでpanicしない

## 主な対象

src/objects/pack/index.rs、src/error.rs、tests/pack_index_test.rs

## 共通方針

Pure Rustと最小依存を維持する。API名は実装時に既存設計と整合させる。新規公開APIは文書化し、変更に対応するテストを追加する。初期リリース対象はSHA-1、loose/pack、loose refs/packed-refsとする。3-way merge・リモート通信・worktree・shallow/partial clone・alternates・reftableの対応拡張は別計画とする。

## 参照

- [実装計画一覧](zerogit-issues.md)
- [Git pack仕様](https://git-scm.com/docs/gitformat-pack)
- [Gitリポジトリ構造](https://git-scm.com/docs/gitrepository-layout)


## GitHub

- Issue: https://github.com/siska-tech/zerogit/issues/9
- 親計画: https://github.com/siska-tech/zerogit/issues/6
- 依存Issue: https://github.com/siska-tech/zerogit/issues/7
