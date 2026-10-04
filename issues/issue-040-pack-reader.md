# Issue #040: Packfileの通常オブジェクトとdelta復元

## 基本情報

| 項目 | 内容 |
|---|---|
| Phase | 3: KazeNhanh向け読み取り・差分 |
| 優先度 | 必須・高 |
| 依存（ローカル計画ID） | 039 |
| ステータス | 実装済み（ローカル・未コミット） |

## 背景・目的

idxから得た位置にあるCommit・Tree・Blob・Tagを復元する。deltaを含めてPack読み取りの実用範囲とする。

## タスク

- [x] SHA-1のpack v2/v3を対象にヘッダ、型、可変長サイズ、zlibを解析する
- [x] OFS_DELTAとREF_DELTA、複数段deltaの復元を実装する
- [x] REF_DELTAの基底取得をストアに委譲できる構造にする
- [x] pack/idx対応checksum、復元OID、サイズ、範囲を検証する。全pack検証のタイミングを定め毎回全走査を避ける
- [x] 展開量・復元深度・循環検出の上限を設け、上限超過をエラーにする

## 受け入れ条件

- [x] 通常4種類、両delta形式、多段deltaの内容がgit cat-fileと一致する
- [x] 不正opcode、基底不足、範囲外copy、切り詰め、過大サイズを安全に拒否する
- [x] pack version未対応を空の結果や不存在として扱わない

## 主な対象

src/objects/pack/reader.rs、src/objects/pack/delta.rs、src/infra/compression.rs、src/infra/hash.rs、src/error.rs、tests/pack_reader_test.rs

## 実装メモ

- 検証タイミング: open時はヘッダ・件数・trailerとidxのpack checksumの一致・エントリ配置（先頭12、trailer手前まで連続）のみ確認する。読み取り時は触れたエントリのCRC32とzlibの宣言サイズ・入力消費、復元OIDを確認する。pack全体のSHA-1と全オブジェクトの読み取りは`PackFile::verify()`を明示的に呼んだ時のみ行う。
- 上限: `PackLimits`の既定値はオブジェクト1GiB、delta深度10,000。加えて宣言サイズは圧縮長×1032+64を超えられない。循環はpack内のoffset訪問済み集合で検出する。
- REF_DELTAの基底がpack外の場合は`read_with_resolver`のresolverへ委譲し、残り深度を渡す。pack間をまたぐ循環は041でストア側が残り深度を引き継ぐことで上限内に収める。
- delta復元結果はpackごとのoffsetキーのFIFOキャッシュ（`PackLimits::delta_cache_size`、既定32MiB、0で無効）に保持し、チェーンの途中から再開する。各結果のチェーン深度も保持するので、深度上限の判定はキャッシュの有無に左右されない。容量の1/4を超えるオブジェクトと、pack外の基底から作った結果はキャッシュしない。
- MSRV（1.70）: Cargo.lockは未コミットのため、CIではstableのCargoのMSRV対応リゾルバ（`CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback`）でlockを生成してからビルドする。ローカルで1.70を使う場合も同じ手順でlockを作る。

## 共通方針

Pure Rustと最小依存を維持する。API名は実装時に既存設計と整合させる。新規公開APIは文書化し、変更に対応するテストを追加する。初期リリース対象はSHA-1、loose/pack、loose refs/packed-refsとする。3-way merge・リモート通信・worktree・shallow/partial clone・alternates・reftableの対応拡張は別計画とする。

## 参照

- [実装計画一覧](zerogit-issues.md)
- [Git pack仕様](https://git-scm.com/docs/gitformat-pack)
- [Gitリポジトリ構造](https://git-scm.com/docs/gitrepository-layout)


## GitHub

- Issue: https://github.com/siska-tech/zerogit/issues/10
- 親計画: https://github.com/siska-tech/zerogit/issues/6
- 依存Issue: https://github.com/siska-tech/zerogit/issues/9
