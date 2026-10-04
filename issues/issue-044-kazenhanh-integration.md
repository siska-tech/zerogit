# Issue #044: KazeNhanh向け文書差分フローと初期リリース条件を整備

## 基本情報

| 項目 | 内容 |
|---|---|
| Phase | 3: KazeNhanh向け読み取り・差分 |
| 優先度 | 必須・高 |
| 依存（ローカル計画ID） | 041、042、043 |
| ステータス | 実装済み（ローカル・未コミット。CIはpush後に確認） |

## 背景・目的

コミット確定→TreeDiff→選択ファイルの旧新Blob→行差分の利用フローを、zerogit側の公開API・利用例・結合テストとして固定する。

## タスク

- [x] 比較開始時にOIDを固定し、一覧取得では全Blobを読み込まず詳細表示時に取得する利用例を追加する
- [x] 追加削除の片側不在、リネームの旧新パス、mode変更、symlink、gitlinkを明示的に扱う
- [x] 初期コミットは空Tree比較、マージは第一親を既定とし、別親は明示選択したTree比較を例示する
- [x] README・API文書・設計書に対応形式と制限を反映する
- [ ] Windows/macOS/Linux、Rust stable/1.70で既存CI・fmt・clippyを実施する（ローカルのWindowsでstable/1.70・fmt・clippyは確認済み。CIはpush後）
- [x] 代表fixtureで読み取り時間とメモリを測定し、処理上限の初期値と根拠を記録する

## 受け入れ条件

- [x] pack+packed-refsを含むリポジトリで変更一覧、旧新文書、行番号付き差分を取得できる
- [x] 参照更新中でも固定したOID間の比較が維持される
- [x] 破損・未対応・処理省略を空の履歴や差分として表示させない結果設計を利用例で示す
- [x] 実行時Git CLI依存を追加せず、既存APIの回帰チェックが通る

## 主な対象

examples/document_diff.rs、examples/measure_document_diff.rs、tests/document_flow_test.rs、README.md、CHANGELOG.md、docs/。KazeNhanh本体の変更はこのリポジトリの範囲外

## 実装メモ

- 利用例は`examples/document_diff.rs`（`--commit`/`--parent`/パス指定）。受け入れテストは`tests/document_flow_test.rs`で、pack化とpacked-refsを行ったリポジトリを使う。比較中のブランチ移動・reset・`git gc`・pack-refsの後も、固定したOID間の比較結果が変わらないことを確認している。破損pack・`max_input_size`による省略・gitlinkのBlob読み取り・SHA-256リポジトリが、いずれも空の差分ではなくエラーまたは明示的な結果になることも確認している。
- 測定（`examples/measure_document_diff.rs`、Windows 11、release）の結果、片側にしか現れない行を事前に除外する最適化を行差分に追加した。LCSが変わらないため編集の最小性は保たれ、ランダム試験で確認済み。2万行の全置換は6.5秒から16msになった。最悪ケースは同じ行が多数繰り返される文書の全置換で、5,000行は約1,700万ステップ・約90ms、2万行は約2.7億ステップ・約3.4秒だった。この結果から`max_cost`の既定値は5,000万（最悪でも約0.25秒以内）のまま据え置いた。
- 1,000コミット・50文書のpack（約1MiB）での測定値は、全履歴の走査50〜220ms、全コミットの変更一覧160〜670ms、行差分は1件あたり0.6〜2.5ms、ピークのワーキングセット約46MiB。時間の幅は実行ごとのディスクキャッシュ等によるばらつき。根拠はREADMEの「処理上限の既定値」に記載した。
- 制限: `DiffDelta::path()`はプラットフォームの`PathBuf`なので、Windowsでは`\`区切りになる（既存の仕様。比較は`Path`同士で行う）。READMEに記載した。

## 共通方針

Pure Rustと最小依存を維持する。API名は実装時に既存設計と整合させる。新規公開APIは文書化し、変更に対応するテストを追加する。初期リリース対象はSHA-1、loose/pack、loose refs/packed-refsとする。3-way merge・リモート通信・worktree・shallow/partial clone・alternates・reftableの対応拡張は別計画とする。

## 参照

- [実装計画一覧](zerogit-issues.md)
- [Git pack仕様](https://git-scm.com/docs/gitformat-pack)
- [Gitリポジトリ構造](https://git-scm.com/docs/gitrepository-layout)


## GitHub

- Issue: https://github.com/siska-tech/zerogit/issues/14
- 親計画: https://github.com/siska-tech/zerogit/issues/6
- 依存Issue: https://github.com/siska-tech/zerogit/issues/11, https://github.com/siska-tech/zerogit/issues/12, https://github.com/siska-tech/zerogit/issues/13
