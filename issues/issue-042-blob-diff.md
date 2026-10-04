# Issue #042: 行差分・hunk・旧新行番号のAPIを追加

## 基本情報

| 項目 | 内容 |
|---|---|
| Phase | 3: KazeNhanh向け読み取り・差分 |
| 優先度 | 必須・高 |
| 依存（ローカル計画ID） | なし（041とは独立に実装可能） |
| ステータス | 実装済み（PR #17） |

## 背景・目的

TreeDiffは変更ファイル一覧として維持し、本文比較にBlobDiff・DiffHunk・DiffLine・DiffOptions（名称案）を追加する。

## タスク

- [x] Myers法を第一候補に決定的な行差分を実装し、文脈行数を設定可能にする
- [x] 行番号は1始まり、追加の旧番号・削除の新番号はNoneにする。空区間のhunk開始位置も仕様化する
- [x] 追加・削除の片側不在を空内容として比較し、実在する空Blobとのメタデータ上の違いを保持する
- [x] LF/CRLF・空ファイル・末尾改行なしを保持し、日本語UTF-8を検証する
- [x] 不正UTF-8を暗黙置換しない。初期は明示的な非テキスト結果とし、生bytesを取得可能にする
- [x] バイナリ・入力サイズや計算量上限による省略と差分なしを区別する

## 受け入れ条件

- [x] 差分を適用して新しい内容を再構成できる
- [x] 旧新行番号、先頭末尾の挿入削除、繰り返し行、全置換、改行のみ変更を検証する
- [x] 同じ入力とオプションで同じ結果となり、上限超過時も部分結果を完全結果として返さない

## 主な対象

src/diff/blob.rs、src/diff/mod.rs、src/lib.rs、tests/blob_diff_test.rs

## 実装メモ

- 公開API: `BlobDiff::compute(old: Option<&[u8]>, new: Option<&[u8]>, &DiffOptions)`と`Repository::diff_blobs(old: Option<&Oid>, new: Option<&Oid>, &DiffOptions)`。結果は`BlobDiffContent::{Text(Vec<DiffHunk>), NonText(NonTextReason), Skipped(SkipReason)}`。`DiffLine`は種別・旧新行番号（1始まり、追加の旧番号と削除の新番号はNone）・改行込みの内容を持ち、`text()`で改行を除いた本文、`ending()`でLF/CRLF/なしを返す。
- アルゴリズム: 行をIDに置き換えた線形空間Myers（middle snake）で、編集量は最小になる。変更のかたまりの中は削除を追加より先に並べるよう正規化し、同じ入力とオプションなら常に同じ結果になる。2×文脈行数以内の間隔の変更は同じhunkにまとめる。
- hunkの開始位置はunified diffの慣例に従う。片側の範囲が空のときは直前の行番号で、ファイル先頭なら0（例: 先頭への挿入は`@@ -0,0 +1,1 @@`）。
- 片側不在は空内容として比較し、`old_exists()`/`new_exists()`で実在する空Blobと区別する。`is_identical()`はbytes比較で、非テキストや省略の結果でも正確。
- 判定順: サイズ上限 → NULを含む → 不正UTF-8 → 行差分（計算量上限）。不正UTF-8を置換して比較することはしない。上限を超えた場合は`Skipped`で、部分的な結果は返さない。
- 既定値: 文脈3行、片側8MiB、計算量5,000万ステップ（目安として「行数の合計×変更行数」程度）。数千行規模の全置換は既定値で`TooComplex`になりうる。妥当な初期値は044の計測で見直す。
- 検証: ランダムな500組の入力で、復元・LCSから計算した最小の追加削除数・決定性を確認した。実リポジトリでは`git diff --numstat --minimal`との一致と復元を確認した（loose・pack両方）。

## 共通方針

Pure Rustと最小依存を維持する。API名は実装時に既存設計と整合させる。新規公開APIは文書化し、変更に対応するテストを追加する。初期リリース対象はSHA-1、loose/pack、loose refs/packed-refsとする。3-way merge・リモート通信・worktree・shallow/partial clone・alternates・reftableの対応拡張は別計画とする。

## 参照

- [実装計画一覧](zerogit-issues.md)
- [Git pack仕様](https://git-scm.com/docs/gitformat-pack)
- [Gitリポジトリ構造](https://git-scm.com/docs/gitrepository-layout)


## GitHub

- Issue: https://github.com/siska-tech/zerogit/issues/12
- 親計画: https://github.com/siska-tech/zerogit/issues/6
