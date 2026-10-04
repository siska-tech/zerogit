# KazeNhanh向け読み取り基盤・行差分の実装計画

## 目的

Packfileとpacked-refsを含むSHA-1リポジトリから変更ファイルと変更前後の文書を取得し、旧新行番号付きの差分をKazeNhanhで利用できるようにする。

## 実装順序

1. ObjectStoreへの読み取り経路統一とpacked-refs対応。
2. idx、packとdelta復元、全APIへの統合。
3. 行差分APIと完全一致リネームのmode修正。
4. 文書差分の利用例・結合テスト・リリース検証。
5. 初期リリース後に任意の類似度リネーム検出。

行差分とリネームmode修正はPack対応と独立に実装できる。各項目は独立したPRとしてレビューする。ローカル計画ID（037〜045）とGitHubのIssue番号は別体系。

## 初期リリースの完了条件

- [x] loose/pack/複数packとpacked-refsの組合せで履歴・変更一覧・旧新文書を取得できる。
- [x] 行差分、旧新行番号、追加削除・改行・バイナリ・処理省略を適切に扱える。
- [x] 不存在・破損・未対応形式を区別し、黙った情報欠落を起こさない。
- [x] 既存の読み書きAPIの回帰確認が通る。
- [ ] Windows/macOS/LinuxおよびRust stable/1.70のCI、fmt、clippyが通る。（ローカルのWindowsでstable/1.70・fmt・clippyは確認済み。CIはpush後に確認）
- [x] 対応範囲・制限・利用例と性能測定結果が文書化されている。

## 対象外

類似度リネームは後続Issueとして追跡する。3-way merge、リモート通信、SHA-256、worktree、shallow/partial clone、alternates、reftableの対応拡張は別計画とする。製品実行時のGit CLI依存は追加しない。KazeNhanh本体のコード変更はこのリポジトリの作業範囲外。

## 個別Issue

親Issue: https://github.com/siska-tech/zerogit/issues/6

| ローカル計画ID | GitHub Issue | 優先度 | 依存（ローカル計画ID） |
|---|---|---|---|
| [037](issue-037-object-store.md) | [#7](https://github.com/siska-tech/zerogit/issues/7) オブジェクト読み取り経路をObjectStoreへ統一 | 必須・高 | なし |
| [038](issue-038-packed-refs.md) | [#8](https://github.com/siska-tech/zerogit/issues/8) packed-refsの参照解決・列挙と既存書き込み操作の保護 | 必須・高 | なし |
| [039](issue-039-pack-index.md) | [#9](https://github.com/siska-tech/zerogit/issues/9) Pack index v2の解析とOID検索 | 必須・高 | 037 |
| [040](issue-040-pack-reader.md) | [#10](https://github.com/siska-tech/zerogit/issues/10) Packfileの通常オブジェクトとdelta復元 | 必須・高 | 039 |
| [041](issue-041-pack-integration.md) | [#11](https://github.com/siska-tech/zerogit/issues/11) Pack読み取りを全APIへ統合しrepack前後の互換性を検証 | 必須・高 | 037、038、040 |
| [042](issue-042-blob-diff.md) | [#12](https://github.com/siska-tech/zerogit/issues/12) 行差分・hunk・旧新行番号のAPIを追加 | 必須・高 | なし（041とは独立に実装可能） |
| [043](issue-043-rename-modes.md) | [#13](https://github.com/siska-tech/zerogit/issues/13) 完全一致リネームで旧新modeを保持する | 必須・中 | なし |
| [044](issue-044-kazenhanh-integration.md) | [#14](https://github.com/siska-tech/zerogit/issues/14) KazeNhanh向け文書差分フローと初期リリース条件を整備 | 必須・高 | 041、042、043 |
| [045](issue-045-similarity-renames.md) | [#15](https://github.com/siska-tech/zerogit/issues/15) 類似度による編集を伴うリネーム検出 | 後続・中（初期リリースの必須条件外） | 042、043、044 |
