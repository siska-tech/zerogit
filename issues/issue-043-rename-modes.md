# Issue #043: 完全一致リネームで旧新modeを保持する

## 基本情報

| 項目 | 内容 |
|---|---|
| Phase | 3: KazeNhanh向け読み取り・差分 |
| 優先度 | 必須・中 |
| 依存（ローカル計画ID） | なし |
| ステータス | 実装済み（PR #17） |

## 背景・目的

現在のDiffDelta::renamedは両側に同じmodeを設定するため、移動と権限変更の併発で情報を失う。

## タスク

- [x] 旧新OID・modeを個別に保持できる内部生成経路を追加し、既存公開コンストラクタとの互換性を維持する
- [x] 同一OID候補の対応付けを決定的にし、一対一対応を保証する
- [x] 通常ファイル・symlink・gitlinkの型の違いを考慮し、不適切なリネーム対応を防ぐ

## 受け入れ条件

- [x] 同一内容の移動と実行権限変更でold_mode/new_modeが正しい
- [x] 同一内容の複数追加削除で結果が決定的となる
- [x] modeのみ変更、通常の追加削除、既存の完全一致リネームが回帰しない

## 主な対象

src/diff/mod.rs、tests/rename_test.rs

## 実装メモ

- `DiffDelta::renamed`（非公開）は旧modeと新modeを別々に受け取る。公開APIの変更はない。
- 対応付けの規則: 追加された側をパス順に処理し、同じOIDかつ同じ種類の、まだ対応付いていない削除された側から選ぶ。ファイル名（basename）が同じものを優先し、次にパス順で最初のものを選ぶ。一対一で、入力の順序によらず結果は同じになる。対応付かなかったものは追加・削除のまま残る。
- 種類: 通常ファイルと実行ファイルは同じ種類として扱う（移動と同時に実行権限が変わっても旧新modeを保持する）。symlinkはsymlink同士、gitlinkはgitlink同士だけを対応付ける。
- 検証: 移動と実行権限変更の同時発生、modeのみの変更、symlinkとgitlinkについて、`git diff --raw -M100%`と旧新パス・旧新modeが一致することを確認した。

## 共通方針

Pure Rustと最小依存を維持する。API名は実装時に既存設計と整合させる。新規公開APIは文書化し、変更に対応するテストを追加する。初期リリース対象はSHA-1、loose/pack、loose refs/packed-refsとする。3-way merge・リモート通信・worktree・shallow/partial clone・alternates・reftableの対応拡張は別計画とする。

## 参照

- [実装計画一覧](zerogit-issues.md)
- [Git pack仕様](https://git-scm.com/docs/gitformat-pack)
- [Gitリポジトリ構造](https://git-scm.com/docs/gitrepository-layout)


## GitHub

- Issue: https://github.com/siska-tech/zerogit/issues/13
- 親計画: https://github.com/siska-tech/zerogit/issues/6
