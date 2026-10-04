# Issue #045: 類似度による編集を伴うリネーム検出

## 基本情報

| 項目 | 内容 |
|---|---|
| Phase | 3: KazeNhanh向け読み取り・差分 |
| 優先度 | 後続・中（初期リリースの必須条件外） |
| 依存（ローカル計画ID） | 042、043、044 |
| ステータス | 実装済み（PR #17） |

## 背景・目的

初期リリースでは編集を伴う移動を追加・削除で表現し、その後に類似度リネームを任意機能として追加する。

## タスク

- [x] 無効・完全一致・類似度の検出オプションを設け、既定の完全一致を維持する
- [x] 類似度定義・しきい値・候補数上限・同点時の決定規則を文書化する
- [x] サイズや型による候補の絞り込みを行い、一対一対応と計算量上限を保証する
- [x] 検出上限に達した場合は追加削除を保持し、検出省略が識別できるようにする

## 受け入れ条件

- [x] 移動と編集、同点候補、空ファイル、大量候補、バイナリを検証する
- [x] 未検出でも追加削除情報を失わず、入力順序で結果が変わらない
- [x] 旧新OID・mode・パスを保持し、設定無効時と既定動作が回帰しない

## 主な対象

src/diff/rename.rs、src/diff/mod.rs、src/lib.rs、tests/rename_test.rs

## 実装メモ

- API: `Repository::diff_trees_with_options`/`commit_diff_with_options`と`RenameOptions`（`RenameDetection::{Off, Exact, Similar}`、`threshold`、`max_pairs`、`max_file_size`）。既定は`Exact`で、既存の`diff_trees`/`commit_diff`は結果が変わらない。
- 類似度の定義、対象、決定規則、上限の詳細は、`docs/zerogit-interface.md` 2.25と`docs/zerogit-detailed-design.md` 9.4に記載した。既定値はしきい値50%（Gitと同じ）、10万組、1ファイル1MiB。
- 上限に達した分は追加・削除のまま残り、`TreeDiff::rename_limits()`で`TooManyPairs`/`FileTooLarge`として識別できる。`DiffDelta::similarity()`は完全一致で100、類似度リネームでそのスコアを返す。
- 作業ツリーとの差分（`diff_index_to_workdir`など）は、従来どおり完全一致のみ。
- 検証: 単体テストで、類似度の値、編集を伴う移動（旧新OID・modeの保持）、同点、類似度とファイル名の優先順位、空ファイル・バイナリ・symlink、上限、入力の逆順でも同じ結果になることを確認した。結合テストでは、対応付けが`git diff -M50%`と一致すること、既定・Offモードで結果が変わらないこと、pack化後も同じ結果になることを確認した。

## 共通方針

Pure Rustと最小依存を維持する。API名は実装時に既存設計と整合させる。新規公開APIは文書化し、変更に対応するテストを追加する。初期リリース対象はSHA-1、loose/pack、loose refs/packed-refsとする。3-way merge・リモート通信・worktree・shallow/partial clone・alternates・reftableの対応拡張は別計画とする。

## 参照

- [実装計画一覧](zerogit-issues.md)
- [Git pack仕様](https://git-scm.com/docs/gitformat-pack)
- [Gitリポジトリ構造](https://git-scm.com/docs/gitrepository-layout)


## GitHub

- Issue: https://github.com/siska-tech/zerogit/issues/15
- 親計画: https://github.com/siska-tech/zerogit/issues/6
- 依存Issue: https://github.com/siska-tech/zerogit/issues/12, https://github.com/siska-tech/zerogit/issues/13, https://github.com/siska-tech/zerogit/issues/14
