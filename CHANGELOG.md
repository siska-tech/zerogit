# Changelog

このプロジェクトは [Keep a Changelog](https://keepachangelog.com/ja/1.0.0/) に準拠し、
[Semantic Versioning](https://semver.org/lang/ja/) を採用しています。

## [Unreleased]

### Added
- `Repository::reflog_expire(expire, expire_unreachable)`と`ReflogExpiry`（`Never`・`Before(time)`・`All`、`ReflogExpiry::parse()`）: `git reflog expire --all`と同じく、`.git/logs/`のすべてのreflogから古いエントリを消し、消した数を返す。`expire`より古いエントリと、`expire_unreachable`より古く、古い値か新しい値が参照の現在の値から到達できないコミットのエントリを消す（HEADは`refs/`のどの参照からも到達できないもの。注釈付きタグはコミットまでたどり、コミットでない・存在しないオブジェクトは到達可能とみなす）。`None`は設定から取る: 参照に最初に一致する`gc.<pattern>.reflogExpire`・`gc.<pattern>.reflogExpireUnreachable`（`*`は`/`にも一致。パターンに設定のないほうは期限なし）、なければ`gc.reflogExpire`・`gc.reflogExpireUnreachable`（既定90日・30日）。パターンに一致しない`refs/stash`は期限切れにしない。値は`never`・`now`（`all`。未来の日時のエントリも含む）・`90.days.ago`・日時。Gitの既定（`--rewrite`なし）と同じく、残るエントリはそのまま保ち、空になったreflogは空のファイルとして残す。参照のロックとreflogのロックをGitと同じく取り、すべてのロックを取ってから書き換えるので、ロックされていれば（`Error::Locked`）何も変更しない (#74)
- `RepackSummary::new_deltas()`: `repack()`が新しく作ったdeltaの数 (#75)
- `Repository::pack_objects_with(wants, haves, &PackObjectsOptions)`と`PackObjectsOptions`（`thin`・`ofs_delta`）: `git pack-objects --revs`と同じく、送るpackをdeltaで作る。`thin`では、送るコミットが積み重なる`haves`のコミットのファイルとtreeもdeltaのbaseにする（受け手が補完するthin pack。`--thin`）。`ofs_delta`ではpackの中のbaseをoffsetで示す（`--delta-base-offset`） (#75)
- `zerogit-remote`: `Transport::push_pack_options()`（既定の実装あり）: pushで送るpackの作り方。Gitのプロトコルではサーバーが`no-thin`を示さなければthin pack、`ofs-delta`を示せばOFS_DELTAにする。ローカルのトランスポートは両方を使う (#75)
- `Repository::describe(&DescribeOptions)`（HEAD）・`describe_revision(revision, &DescribeOptions)`と`DescribeOptions`・`Description`: `git describe`と同じく、到達できる最も近いタグとそこからのコミット数・短縮OIDでコミットを名付ける（`v1.0-3-gabc1234`、タグの付いたコミットはタグ名だけ）。注釈付きタグ（`tags()`で軽量タグ、`all()`で任意の参照も）、`long()`・`abbrev()`（`core.abbrev`。既定はGitと同じくpackのオブジェクト数から決め、一意になるまで伸ばす）・`always()`・`first_parent()`・`candidates()`・`patterns()`（`--match`）・`exclude()`・`dirty()`（追跡中のファイルの変更があれば接尾辞を付ける）。同じコミットに複数のタグがあれば、注釈付きを優先し、注釈付き同士ではタグ付けの新しいものを使う。`Error::NoDescription`（軽量タグなら名付けられたかを含む） (#77)
- `Repository::mailmap()`と`Mailmap`（`parse()`・`add()`・`resolve(name, email)`・`resolve_signature()`）: `git check-mailmap`と同じく、`.mailmap`の4つの形式で名前・メールアドレスを正規化する。メールアドレスと名前は大文字小文字を区別せず（ASCII）、名前とアドレスの組の対応がアドレスだけの対応より優先し、後の行が前の行を上書きする。読む順はGitと同じく、作業ツリーの`.mailmap`（シンボリックリンクは追わない）、`mailmap.blob`（bareリポジトリでは既定で`HEAD:.mailmap`）、`mailmap.file`。`log()`などの結果は変えない（コミットを書かれたまま返す）ので、表示の際に`resolve_signature()`で適用する (#77)
- `Repository::blame(path, &BlameOptions)`と`Blame`・`BlameLine`・`BlameHunk`・`BlameOptions`・`DiffAlgorithm`: `git blame`と同じく、ファイルの各行を最後に変えたコミット（とその版での行番号・パス）を求める。親に同じ行がある限り過去へ渡し、マージは親を順に試し、親にないファイルはリネーム元（同じ内容、なければ50%以上似ているファイル）を探す。`revision()`・`first_parent()`（`--first-parent`）・`lines()`（`-L`）・`diff_algorithm()`（`--diff-algorithm`: myers・minimal・histogram）・`follow_renames()`。`-M`・`-C`は対象外。代表的な履歴と、このリポジトリの241ファイル中240ファイルで`git blame --line-porcelain`と全行が一致する。大きな書き換え（1か所で数百行以上の変更）をまたぐ行は、Gitの既定のdiffが最小でなくなるため異なることがある (#77)
- `docs/diff-compat-research.md`: 行diffを`git diff -U0`・`git blame`とそろえるために、Gitの出力だけから実験で求めた規則（長い共通の末尾の扱い、よく現れる行を除く条件、移動できる変更のまとまりの置き場所）と、その実験。`src/diff/compat_tests.rs`（保存した実ファイルの組とGit 2.55の出力、乱数入力との比較）で確かめる (#77)
- `Repository::blob_reader(revision)`・`object_reader(revision)`と`ObjectReader`（`std::io::Read`、`object_type()`・`size()`・`is_streamed()`）: オブジェクトの内容をストリームで読む。looseオブジェクトと、packに丸ごと格納されたオブジェクト（Gitは`core.bigFileThreshold`より大きいファイルをこう格納する）は、読みながら展開するため、メモリ使用量はファイルの大きさによらず、packの読み取りの上限（`PackLimits::max_object_size`、既定1 GiB）もかからない。deltaのオブジェクトは従来どおりメモリ上で復元する。最後まで読んだ時点で、内容がオブジェクトIDと（packではCRC-32とも）一致することを確かめ、異なれば`std::io::ErrorKind::InvalidData`のエラーにする (#78)

### Changed
- `gc()`が`git gc`と同じく、`pack_refs()`の後・`repack()`の前にreflogを期限切れにする（`reflog_expire(None, None)`）。reflogからだけ到達できていた古いオブジェクトも、pruneの期限を過ぎれば消える。`gc.*reflogExpire*`の値が不正なら何も変更せずに`Error::InvalidDate`を返す (#74)
- `repack()`（と`gc()`）がdeltaを計算する: 既存のpackのdeltaを再利用できないオブジェクト（looseオブジェクトなど）を、`git pack-objects`と同じく型・ファイル名・サイズで並べ、前の`pack.window`個（既定10）のオブジェクトとのdeltaを試し、最も小さいものを使う。deltaの連鎖は`pack.depth`（既定50）まで、`core.bigFileThreshold`（既定512 MiB）より大きいオブジェクトはdeltaにしない。delta（コピーと挿入の命令列）の作成はクリーンルームで実装し、作ったdeltaは元のオブジェクトを再現することを確かめてから書く。READMEの計測（zerogitが書いた2,000コミット）で、`gc()`後のサイズが7.6 MBから1.5 MB（`git gc`は1.7 MB）になった (#75)
- `pack_objects()`（push・fetchで送るpack）がdeltaを使う（`repack()`と同じ探索。packにあるdeltaは、baseも送るなら再利用する）。baseはIDで示す（REF_DELTA。どの受け手も読める） (#75)
- `zerogit-remote`: pushが`git push`と同じくthin packを送る。サーバーにあるファイルを少し変えたpushでは、そのファイルのdeltaだけを送る（約200 KBのファイルの変更で3 KB未満） (#75)
- マージ（histogram diff）がMyers diffにフォールバックする領域で、よく現れる行を除く条件をGitの出力に合わせた（行自身の数え方の誤り）(#77)
- `core.bigFileThreshold`（既定512 MiB）より大きく、変換（改行コード・`text`属性）のないファイルを、Gitと同じくストリームで扱う。`add()`・`add_all()`・`stash_save()`（未追跡ファイル）は読みながらハッシュと圧縮をしてlooseオブジェクトに書き、`checkout()`・`restore()`・マージ・`stash_apply()`は展開しながら作業ツリーに書くため、メモリ使用量はファイルの大きさによらない（1.1 GBのファイルで、add・restore・checkoutのヒープ使用量のピークは0.5 MB以下）。読む間にファイルの大きさが変わればエラーにし、何も書かない。変換のあるファイルとシンボリックリンクは従来どおりメモリ上で扱う (#78)
- 同じく`core.bigFileThreshold`より大きく変換のないファイルを、status（と作業ツリーとの差分、checkoutなどの上書き確認）は読みながらハッシュし、tracked fileの変更を含む`stash_save()`は変更のあるファイルだけをストリームで書く。`repack()`・`gc()`・`pack_objects()`は、しきい値より大きいオブジェクトをpackへ展開・圧縮しながら書き、既存のpackから丸ごとコピーする場合もストリームでコピーする（CRC-32は最後に確かめる）。到達可能性の走査は、indexにあるblobを読まずにヘッダーから型を判定する。これらにより、packの読み取りの上限（1 GiB）を超えるファイルがあってもgcでき、1.1 GBのファイルでgcのヒープ使用量のピークは0.5 MBだった (#78)
- `diff_blobs()`が、`DiffOptions::max_input_size`を超える側のblobを読まずに`Skipped(InputTooLarge)`を返す（結果は従来と同じ）。類似度によるリネーム検出（diff・merge・blame）も、`RenameOptions::max_file_size`を超えるblobを読まない (#78)

### Development
- `src/repository.rs`（3,785行）を機能ごとのモジュールに分けた（約1,000行に）。オブジェクトの読み取り（`object_access.rs`）、ステージング（`staging.rs`）、checkout（`checkout.rs`）、ブランチ・タグ（`refs/branch_ops.rs`・`refs/tag_ops.rs`）、参照の更新の共通処理（`refs/update.rs`）、log（`log.rs`）、status（`status.rs`）、commitとtreeの作成（`commit.rs`）、reflog（`refs/reflog.rs`）。公開API（名前・シグネチャ・ドキュメントとそのパス）は変わらない (#76)

### Notes
- Git 2.55で確認したところ、`gc.reflogExpire`・`gc.reflogExpireUnreachable`がどちらも未設定の場合、`git reflog expire`は文書と異なり、到達可能なエントリも30日で消した。zerogitは文書どおりの既定（90日・30日）を使う。両方を設定するか`--expire`・`--expire-unreachable`を指定した場合の結果は一致する (#74)

## [0.9.0] - 2026-10-06

日常の操作をさらにそろえた: cherry-pick・revert、gc・repack・prune・pack-refs、clean、logの並び順（`--date-order`・`--topo-order`・`--reverse`）と`git log --graph`の列の割り当て。`zerogit-remote`は0.5.0として、zerogit 0.9に追従する。

互換性に影響する変更（下記「Changed」）: `log()`の既定の並びと`LogOptions::since()`・`until()`の判定が、Gitと同じcommitterの日時になった。`Error`が`#[non_exhaustive]`になり、`match`には`_`の腕が必要になった。`delete_branch()`・`delete_tag()`は`packed-refs`にある参照も削除する。

### Added
- cherry-pick・revert: `Repository::cherry_pick(&commits, &CherryPickOptions, ...)`・`Repository::revert(&commits, ...)`と、それぞれの`*_continue`・`*_skip`・`*_abort`。`git cherry-pick`・`git revert --no-edit`と同じく、コミットを指定順に（重複は1回だけ）3-way merge（リネームの追従を含む）で適用する。cherry-pickはauthor・メッセージを元のまま保ち、`CherryPickOptions::record_origin`で`(cherry picked from commit <oid>)`を追記する（`-x`。trailerの後には空行を入れない）。revertのメッセージはGitと同じ`Revert "<subject>"`（revertのrevertは`Reapply "..."`）と`This reverts commit <oid>.`。結果は`PickOutcome`（`Completed`・`Conflicts`・`Empty`）で、コンフリクトと、変更が空になるコミット（すでに入っている変更）はGitと同じく止まる。状態は`CHERRY_PICK_HEAD`・`REVERT_HEAD`・`MERGE_MSG`と、複数コミットでは`.git/sequencer/`（`todo`・`head`・`abort-safety`・`opts`）にGitの形式で書き、zerogitとGitのどちらからでも続行・スキップ・中止できる。中止はGitと同じく`reset --merge`で戻し、無関係なローカルの変更は残す。マージコミット（`-m <parent>`）は`Error::UnsupportedCherryPick`で対象外 (#62)
- `Error::CherryPickInProgress`・`Error::NoCherryPickInProgress`・`Error::RevertInProgress`・`Error::NoRevertInProgress`・`Error::UnsupportedCherryPick` (#62)
- `Repository::pack_refs()`: `git pack-refs --all`と同じく、looseの参照を`packed-refs`にまとめ（ソート済み、注釈付きタグには`^`の行。ヘッダもGitと同じ）、looseのファイルと空になったディレクトリを消す。シンボリック参照、作業ツリーごとの参照（`refs/bisect/`など）、存在しないオブジェクトを指す参照はlooseのまま残す。途中で変わった・ロックされているlooseの参照は消さない (#63)
- `Repository::repack()`と`RepackSummary`: `git repack -a -d`と同じく、到達可能なオブジェクトを1つのpack（v2、idx v2）にまとめ、古いpackと、packに入ったlooseオブジェクト（空になったファンアウトのディレクトリも）を消す。到達可能の範囲は参照・HEAD・reflog・index、止まったmerge・rebase・cherry-pick・fetchの状態ファイル（`ORIG_HEAD`・`MERGE_HEAD`・`FETCH_HEAD`など）。古いpackでdeltaだったオブジェクトは、baseも新しいpackに入るなら圧縮済みのdeltaをそのまま使う（新しいdeltaは計算しない。looseオブジェクトはそのまま格納する）。`.keep`のあるpackには触れない。到達できないオブジェクトが古いpackにあれば、`git repack -A`と同じくlooseとして書き出し、消さない。packは一時ファイルに書いてからrenameし、indexを最後に置くので、読み手は常にすべてのオブジェクトを読める。到達可能なオブジェクトが欠けている場合は何も消さずにエラーを返す (#63)
- `Repository::prune(older_than)`: `git prune [--expire <time>]`と同じく、到達できないlooseオブジェクトを消す。期限を指定すると、それより新しいオブジェクトと、新しい到達できないオブジェクトが参照するオブジェクトは残す（書き込み中の操作のオブジェクトを消さない）。packに入っているlooseオブジェクトと、期限を過ぎた中断済みの一時ファイルも消す (#63)
- `Repository::gc()`と`GcSummary`: `git gc`と同じく、`pack_refs()`・`repack()`・`prune()`を続けて行う。pruneの期限は`gc.pruneExpire`（既定`2.weeks.ago`、`now`・`never`、`<n>.<単位>.ago`、日時）。実行中は`gc.pid.lock`を持ち、`git gc`と同時に動かない。Gitと異なり、reflogは期限切れにせず、到達できないオブジェクトはcruft packではなくlooseで残す (#63)
- `Repository::gc_auto()`: `git gc --auto`と同じ条件（looseオブジェクトが`gc.auto`（既定6700。`objects/17/`から推定）より多い、または`.keep`のないpackが`gc.autoPackLimit`（既定50）より多い）のときだけ`gc()`を行う。`gc.auto = 0`で無効。書き込み操作の中では自動実行しない (#63)
- `examples/measure_gc.rs`: zerogitが書いた履歴でのgc前後のオブジェクト数・サイズ・`log()`・`status()`の計測。結果はREADMEに記載 (#63)
- `Repository::clean(&CleanOptions)`、`CleanOptions`、`CleanIgnored`: `git clean -f`と同じく、作業ツリーから未追跡ファイルを消し、消したもの（`dry_run`では消すもの、`git clean -n`が示すパス）をパス順で返す。`directories`（`-d`）、`ignored`（`Keep`・`Include`=`-x`・`Only`=`-X`）、`paths`（pathspec）、`dry_run`（`-n`）。ディレクトリは、消すファイルだけなら丸ごと、残すファイルがあれば中身を個別に消す。pathspecがディレクトリを指せば`-d`なしでも対象になり、globはその固定部分が届くディレクトリの中まで照合する。`-X`ではignoreされたディレクトリの中を見ずに丸ごと消す（いずれもGitと同じ）。追跡中のファイル（ignoreのパターンに一致しても）、ネストしたリポジトリ（`.git`を持つディレクトリ）とその中身、`.git`は消さない。読み取り専用のファイルも消す。bareリポジトリでは`Error::InvalidPathOperation`で何も消さない (#64)
- `LogOrder`と`LogOptions::order()`: `git log`の並び順。`Default`（Gitの既定）、`Date`（`--date-order`）、`Topo`（`--topo-order`）。`Date`・`Topo`はGitの`sort_in_topological_order`と同じく、親が子より先に出ない順で、履歴全体を読んでから返す。`LogOptions::reverse()`（`--reverse`。`max_count`・フィルタで選んだ後に逆順にする）。同じ日時・日時の逆転・criss-cross・octopusを含む履歴で、各順と`--first-parent`・`-n`の組み合わせが`git log`と一致する (#65)
- `log::Graph`・`log::GraphRow`: 与えた順のコミットに`git log --graph`と同じ列を割り当てる（Gitの`graph.c`の列の更新と同じ）。各コミットの列、行の前後の線（`columns_before`・`columns_after`）、各親へ続く列、線の対応を返し、文字の描画は利用側に任せる。`*`の位置が`git log --graph`（`--first-parent`、無関係な履歴のマージを含む）と一致する (#65)
- `examples/measure_log.rs`: 大きな履歴での並び順ごとの計測。結果はREADMEに記載 (#65)

### Changed
- 止まったcherry-pick・revertの間の`create_commit_with()`は、`git commit`と同じくそのコミットを確定する（`CHERRY_PICK_HEAD`・`REVERT_HEAD`・`MERGE_MSG`を消す。cherry-pickでは元のauthorを使い、reflogは`commit (cherry-pick):`）。`amend_commit()`・`merge()`は止まったcherry-pickの間はGitと同じく`Error::CherryPickInProgress`で拒否する。mixed・hardの`reset_to()`はcherry-pick・revertの停止状態も終える (#62)
- `delete_branch()`・`delete_tag()`が`packed-refs`にある参照も削除する（looseとpackedの両方にあれば両方）。これまでは`Error::PackedRefDeletionUnsupported`で拒否していた（このエラーはもう返さない）(#63)
- `log()`・`log_with_options()`の既定の並びを、authorの日時からGitと同じcommitterの日時に変えた。あわせて、同じ日時のコミットは辿った順に返し、各コミットを最初に見つけた時点で一度だけキューに入れる（Gitの既定の`git log`と同じ順になる）(#65)
- `LogOptions::since()`・`until()`の判定を、authorの日時からGitと同じcommitterの日時に変えた。`since`より古いコミットからは先をたどらない（既定の順）。`Date`・`Topo`では、Gitと同じく`since`より古いコミットの祖先をすべて除き、日時の範囲外のコミットを除いてから並べる。`--since`・`--until`と各順の組み合わせが`git log`と一致する
- `Error`に`#[non_exhaustive]`を付けた。今後エラーの種類を増やしても互換性を壊さないためで、`match`には`_`の腕が必要になる

### Fixed
- `stash_drop()`で最後のstashを消したとき、`packed-refs`にある`refs/stash`が残り、消したはずのstashが見えていた (#63)
- `LogOptions::since()`・`until()`の`YYYY-MM-DD`が、うるう年の1〜2月に1日ずれていた（閏日の数え方の近似）。UTCの0時として正しく計算する

## [0.8.0] - 2026-10-05

Gitとの互換性を広げた: merge・rebase・stashでのリネームの追従とリネームのコンフリクト、対話的rebase、SSHトランスポートの強化と実サーバーでの検証。`zerogit-remote`は0.4.0として、zerogit 0.8に追従する（`Error::Connection`の追加など）。

### Added
- `zerogit-remote`: SSHクライアントをGitと同じ順で選ぶ: `GIT_SSH_COMMAND`（シェルで実行）、`GIT_SSH`（プログラム）、`core.sshCommand`（シェルで実行。`fetch`・`push`はリポジトリの設定を読む）、`ssh`。PuTTYの`plink`・`tortoiseplink`にはそのポート指定（`-P`）を使う。`ProcessConnector::ssh_command()`、`transport::open_for()` (#36)
- `zerogit-remote`: `Error::Connection`: 接続が応答の前に切れたとき、SSHクライアントの終了状態とメッセージ（認証失敗、ホスト鍵の不一致など）を含めて返す (#36)
- CI: localhostの`sshd`に対してSSH経由のclone・fetch・pushをGitと比べるジョブ（`zerogit-remote/tests/ssh_test.rs`。ほかの環境では`#[ignore]`）(#36)
- 対話的rebase: `Repository::rebase_interactive(upstream, onto, &steps, ...)`、`Repository::rebase_plan(upstream)`（`git rebase -i`が最初に示すpickの一覧）、`RebaseStep`（`Pick`・`Reword`・`Edit`・`Squash`・`Fixup`・`Drop`）。並べ替え・省略（省いたコミットは捨てる）もできる。squashのメッセージはGitのエディタが残す結合メッセージと同じ。`edit`では`RebaseOutcome::Stopped`で止まり、ステージした変更は`rebase_continue`でそのコミットに加わる。状態はGitの形式（`rebase-merge/`のtodo・done・amendなど）で保存し、Gitとzerogitのどちらからでも続行できる。`exec`・`break`・`label`・`reset`・`merge`は対象外 (#34)
- merge（rebase・stashの適用を含む）が、片側でリネームされたファイルを追う（Gitの`ort`と同じ）。完全一致または類似度50%以上のリネームを検出し、もう片側の変更をリネーム先に適用する。コンフリクトマーカーは`HEAD:新しいパス`・`topic:古いパス`のように両側のパスを示す。`merge.renames`（既定は`diff.renames`）が`false`なら追わない。リネームのコンフリクト（片側でリネーム・もう片側で削除、両側で別のパスへのリネーム、同じパスへのリネーム）はGitと同じindexのステージと作業ツリーで記録する。ディレクトリのリネームは検出しない (#35)

### Fixed
- `zerogit-remote`: `-`で始まるホスト名・パスがSSHクライアントのオプションとして解釈されえた（CVE-2017-1000117と同じ問題）。Gitと同じく拒否する (#36)
- `zerogit-remote`: `GIT_SSH_COMMAND`を空白で分割して実行していたため、クォートした引数（空白を含む鍵のパスなど）が扱えなかった。Gitと同じくシェルで実行する（Windowsで`sh`がない場合は従来どおり分割する）(#36)

## [0.7.0] - 2026-10-04

日常の操作をそろえた: リビジョン指定（`rev_parse`）、`reset --soft/--mixed/--hard`、`commit --amend`、`restore`・`rm`・`mv`、変更を持ち越すcheckout。`zerogit-remote`は0.3.0として、zerogit 0.7に追従する。

`checkout`は、変更があると一律に`Error::DirtyWorkingTree`で止まる動作から、Gitと同じく切り替えで失われない変更を持ち越す動作に変わった（下記「Changed」）。

### Added
- `Repository::rev_parse()`: `git rev-parse`と同じくリビジョンを解決する。参照名（Gitと同じ順: `refs/`・`refs/tags/`・`refs/heads/`・`refs/remotes/`・`refs/remotes/<name>/HEAD`）、短縮OID、`@`、`~<n>`・`^<n>`、`^{}`・`^{commit}`・`^{tree}`・`^{blob}`・`^{tag}`・`^{object}`、`<rev>:<path>`、`:<path>`・`:<n>:<path>`（index）、`<ref>@{<n>}`（reflog）、`@{-<n>}`、`@{upstream}`・`@{u}`に対応する。日付、`@{push}`、メッセージ検索（`^{/text}`・`:/text`）、範囲は未対応 (#44)
- `Error::InvalidRevision`: 解決できないリビジョン。理由に問題の部分を含む (#44)
- `Repository::reset_to(revision, ResetMode)`と`ResetMode`（`Soft`・`Mixed`・`Hard`）: `git reset --soft/--mixed/--hard <rev>`と同じくHEAD（ブランチ）を動かす。`ORIG_HEAD`と`reset: moving to <rev>`のreflogを記録し、mixed・hardはマージ中の状態を終える。mixedは内容の変わらないエントリのstat情報を保ち、ほかは作業ツリーから更新する。softはマージ中・コンフリクト中にはGitと同じく拒否する。ロック中などの失敗時は何も変えない (#45)
- `Repository::reset_paths(revision, paths)`: `git reset <rev> -- <paths>`と同じく、ファイルまたはディレクトリ配下のindexのエントリを指定したコミットの内容にする (#45)
- `Repository::restore(paths, &RestoreOptions)`: `git restore [--source=<rev>] [--staged] [--worktree]`。作業ツリー（既定はindexから）・index（既定はHEADから）・両方を復元する。作業ツリーへの書き出しは`checkout`と同じく改行変換・symlink・実行ビットを扱い、indexに新しいstat情報を記録するので、直後の`status()`はファイルを読まずに空になる。コンフリクト中のパスを作業ツリーに戻す指定はGitと同じく拒否する (#46)
- `Repository::remove(paths, &RemoveOptions)`: `git rm [--cached] [-f] [-r]`。Gitと同じく、コミットしていない変更（ステージ済み・作業ツリー）を失う削除は`force`なしでは拒否し、ディレクトリには`recursive`が必要。コンフリクト中のパスは削除で解消できる (#46)
- `Repository::move_path(source, destination)`: `git mv`。ファイル・ディレクトリを作業ツリーとindexで移動し、エントリのstat情報を保つ。移動先が既にある・ディレクトリ自身の中・親ディレクトリがない・移動元が未追跡やコンフリクト中・移動元のファイルがない場合は、Gitと同じく拒否する (#46)
- pathspec: 上記はパス・ディレクトリ（配下すべて）・glob（`*`・`?`・`[...]`。Gitの既定と同じく`/`にもマッチする）・`.`（すべて）を受け付ける。どのファイルにも一致しないpathspecは`Error::PathspecNotMatched` (#46)
- `RestoreOptions`、`RemoveOptions`、`Error::PathspecNotMatched`、`Error::InvalidPathOperation`、`Error::UncommittedChanges` (#46)
- `Repository::amend_commit(message, &CommitOptions)`: `git commit --amend`と同じく、元のコミットの親・author（指定すれば変更可）と、新しいcommitterでコミットを置き換える。メッセージを省略すると元のメッセージをそのまま使う（`--no-edit`）。reflogは`commit (amend):`。Gitと同じく、親と同じtreeになる場合（マージコミットを除く）とマージ中は拒否する (#45)
- `Repository::checkout_with(target, &CheckoutOptions)`と`CheckoutOptions::force`: `git checkout -f`と同じく、ローカルの変更・邪魔になる未追跡ファイル・コンフリクトを捨てて切り替える (#43)

### Changed
- `checkout`は、Gitの2-way merge（`read-tree -m -u`）と同じく、現在のHEADと切り替え先で異なるパスだけを変える。無関係な未追跡ファイルや、切り替えで変わらないパスの変更（ステージ済みを含む）は残したまま切り替える。これまでは変更が1つでもあると`Error::DirtyWorkingTree`で止まっていた。上書き・削除されるファイルに変更があるとき、または無視されていない未追跡ファイルと衝突するときは、`Error::LocalChangesWouldBeOverwritten`でパスを列挙して止まり、何も変えない。無視されたファイルはGitと同じく上書きする。判定はmerge・rebaseと共通 (#43)
- `Repository::commit()`・`tree()`・`blob()`・`object()`がリビジョンを受け付ける（`repo.commit("HEAD~1")`など）。短縮OIDの指定はこれまでどおり動く (#44)
- `checkout`・`merge`・`rebase`の対象の解決を`rev_parse`にそろえた。参照名はGitと同じ順で探す（同名のタグとブランチではタグ、`origin`は`origin/HEAD`）。`checkout`は`-`・`@{-<n>}`がブランチを指すとき、Gitと同じくそのブランチに切り替える (#44)

### Fixed
- 参照名の解決で、ディレクトリ（`origin`に対する`refs/remotes/origin`など）を読もうとしてI/Oエラーになっていた。参照がないものとして扱う (#44)
- HEADを同じコミットへ動かす操作で、Gitと同じく、値の変わらないブランチのreflogには記録しない（ブランチを指すHEADのreflogには記録し、detachedのHEADには記録しない）(#45)
- `refresh_index()`でstat情報を更新したエントリのcache treeを無効にしていた（内容は変わらないので有効なまま保つ）(#45)

## [0.6.0] - 2026-10-04

Gitと並行して安全に使えるようにし（ロックファイル、正しいタイムゾーンとメッセージ）、大きな作業ツリーでのstatusを速くした。`zerogit-remote`は0.2.0として、zerogit 0.6に追従する。

### Added
- `Error::Locked`: 書き込み先のロックファイル（`<path>.lock`）が既にある場合のエラー。メッセージにロックファイルのパスを含む (#39)
- `Repository::create_commit_with()`と`CommitOptions`: 作者・コミッター（日時とタイムゾーンを含む）を別々に指定してコミットする。`allow_empty`（`--allow-empty`）と`allow_empty_message`（`--allow-empty-message`）も指定できる。既定ではGitと同じく、親と同じtreeのコミットと空のメッセージを拒否する (#40)
- `Repository::default_author()`・`default_committer()`: `git commit`と同じく、`GIT_AUTHOR_*`・`GIT_COMMITTER_*`（`*_DATE`はGitの内部形式・ISO 8601・RFC 2822）、`author.*`・`committer.*`、`user.name`・`user.email`、`EMAIL`の順で署名を解決する (#40)
- `Signature::now()`: 現在時刻とローカルのタイムゾーンで署名を作る (#40)
- `Error::EmptyCommitMessage`、`Error::InvalidDate` (#40)
- `Repository::refresh_index()`: 内容が変わっていないファイルのstat情報をindexに書き戻す（`git update-index --refresh`相当）(#41)
- `examples/measure_status.rs`: 大きな作業ツリーで`status()`と`git status`を比べる計測 (#41)

### Changed
- index・参照（HEAD・ブランチ・タグ・`refs/stash`など）・`packed-refs`・config・`MERGE_*`・`ORIG_HEAD`・`rebase-merge/`の書き込みを、Gitと同じロックファイル（`<path>.lock`を排他的に作成して書き込み、renameで置き換える）経由にした。Gitが操作中（ロックあり）なら、zerogitは何も変えずに`Error::Locked`を返す (#39)
- indexを読んで書き戻す操作（`add`・`add_all`・`reset`・`checkout`・`merge`・`stash`・`rebase`など）は、ロックを取ってからindexを読む。`create_commit`はHEADの更新が終わるまでindexのロックを持つ (#39)
- `checkout`は、作業ツリーを変える前にindexとHEADのロックを取る (#39)
- 作成するコミット・注釈付きタグ・stash・merge・rebaseのコミットとreflogに、ローカルのタイムゾーン（夏時間を含む）を記録する。これまでは常に`+0000`だった。オフセットは依存を増やさず、Unixでは`localtime_r`、Windowsでは`SystemTimeToTzSpecificLocalTime`で求める (#40)
- `create_commit`とmergeのコミットメッセージを`git commit -m`と同じく整形する（行末の空白、前後の空行、連続する空行を除き、末尾に改行を付ける）。同じtree・親・署名・日時・メッセージなら、Gitで作ったコミットと同じOIDになる (#40)
- status・detailed_statusと作業ツリーとの比較（checkout・merge・stash・rebaseの確認を含む）は、indexのstat情報（size・mtime・ctime・inode・uid・gid。`core.trustctime`・`core.checkStat`に従う）が一致するファイルを読まない。indexと同じ秒以降に変更されたエントリ（racy-git）とサイズ0に印を付けたエントリは内容で比較する。4,000ファイル・220 MBで`status()`が約1.1秒から約0.13秒になった（`git status`の1.3倍）(#41)
- indexに記録するstat情報をGitと同じにした（Unixではctime・dev・inode・uid・gid、WindowsではGit for Windowsと同じく作成時刻をctimeとする）。indexを書くとき、その秒に変更されたエントリはGitと同じくサイズを0にして、次の読み手に内容を比較させる (#41)

### Fixed
- indexを書き戻すと、Gitが作ったcache tree（`TREE`）とresolve-undo（`REUC`）の拡張が失われていた。どちらも読み込んで保持し、エントリの変更に合わせて更新して書き戻す。cache treeは変更したパスのディレクトリだけを無効にし、`create_commit`は有効な部分のtreeを使い回したうえで、作ったtreeでcache treeを埋めて書き戻す。コンフリクトを`add`（または削除）で解消するとresolve-undoに記録し、`git checkout -m <path>`でコンフリクトを作り直せる。コミット・mergeの中止・`reset_hard`で記録を消す（Gitと同じ）。untracked cache（`UNTR`）など内容を検証できない任意の拡張は、書き込み時に落とす（Gitが作り直す）(#42)
- indexからtreeを作る処理の計算量を、ディレクトリ数の2乗から線形にした (#42)
- `update_reference()`の期待値付き更新が、ロックを取ってから現在値を比べるようになり、compare-and-swapとして正しく働く。同時に更新すると1つだけが成功し、残りは`StaleReference`か`Locked`になる。`create_commit`・merge・rebaseによるHEADの更新も、読んだときから動いていれば`StaleReference`で拒否する。reflogは参照のロックを持っている間に追記する (#39)
- looseオブジェクト・pack・作業ツリーのファイルを書くときの一時ファイル名を、プロセスと呼び出しごとに一意にした。同じファイル（同じオブジェクトなど）を同時に書いても壊れない (#39)

### Development
- テスト用fixtureを、テストが初回に使うときにGitで自動生成するようにした。クリーンなクローンで`cargo test`だけを実行して通る。`create_fixtures.sh`・`create_fixtures.ps1`は削除した (#47)
- CIのcoreのlintを`cargo clippy --all-targets -- -D warnings`にし、`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`を追加した。既存の指摘と壊れたdocリンクを解消した (#47)

## [0.5.0] - 2026-10-04

### Added
- `Error::UnmergedPaths`: indexにコンフリクト（stage 1〜3）が残っている場合のエラー
- `Index::get_stage()`、`Index::has_conflicts()`、`Index::conflicted_paths()`
- `.gitignore`・`.git/info/exclude`・`core.excludesFile`の解釈（否定、ディレクトリ指定、固定、`*`・`?`・`[...]`・`**`、エスケープ、`core.ignoreCase`）。status・`add_all`・作業ツリーとの差分に適用する (#20)
- `Repository::is_ignored()`、`Repository::ignored_files()`、`Repository::add_force()`
- `Error::IgnoredPath`: 無視された未追跡ファイルを`add`した場合のエラー
- 改行コードの変換: `core.autocrlf`（true/input/false）・`core.eol`・`.gitattributes`（`text`、`text=auto`、`-text`、`binary`、`eol=lf|crlf`、旧形式の`crlf`）・`core.attributesFile`・`.git/info/attributes`に従い、`add`/`add_all`・status・作業ツリーとの差分ではCRLF→LF、`checkout`ではLF→CRLFに変換する。`text=auto`の判定とindexにCRLFがあるファイルの扱いはGitと同じ (#21)
- `core.safecrlf=true`で往復できない変換（改行の混在など）を`add`すると`Error::IrreversibleLineEndings`を返す。既定（warn）では変換して続行する
- `filter`（driverが設定されている場合）・`ident`・`working-tree-encoding`属性は未対応として扱い、`add`/`checkout`は`Error::UnsupportedAttribute`を返す。statusと作業ツリーとの差分は、サイズと更新時刻がindexと一致すれば変更なしとし、一致しなければ変更ありとする。driverが設定されていない`filter`はGitと同じく無視する
- `Repository::detailed_status()`: `git status --porcelain=v2`と同じく、index側（HEAD→index）と作業ツリー側（index→作業ツリー）の状態をパスごとに別々に返す。コンフリクトの種類（`UU`・`AA`・`DU`など）、intent-to-add（`.A`）、型の変更（`T`）、ステージ済みの追加後の削除（`AD`）を区別する (#24)
- `DetailedStatus`、`DetailedStatusEntry`、`ChangeState`、`ConflictKind`
- reflogの書き込み: `create_commit`（`commit:`・`commit (initial):`）、`create_branch`（`branch: Created from ...`）、`checkout`（`checkout: moving from ... to ...`）でHEADとブランチのreflogをGitと同じ形式で追記し、`delete_branch`でブランチのreflogを削除する。`core.logAllRefUpdates`に従う。記録する名前とメールは、コミットではコミッター、それ以外では`GIT_COMMITTER_NAME`/`GIT_COMMITTER_EMAIL`、`user.name`/`user.email`の順 (#25)
- `Repository::reflog()`、`ReflogEntry`: reflogの読み取り（新しい順）
- タグの作成・削除: `Repository::create_tag()`（軽量タグ）、`Repository::create_annotated_tag()`（tagオブジェクトを書き込む注釈付きタグ。対象は任意の型で、`type`は対象の実際の型。taggerは`create_commit`の作者と同じく引数で指定し、メッセージは`git tag -m`と同じく整形する）、`Repository::delete_tag()`。既存のタグは上書きしない (#27)
- merge: `Repository::merge()`（fast-forward・3-way merge・コンフリクトの記録）、`Repository::merge_base()`/`merge_bases()`（`git merge-base [--all]`相当）、`Repository::merge_head()`、`Repository::abort_merge()`（`git merge --abort`相当）、`MergeOptions`、`FastForward`（`--ff`/`--ff-only`/`--no-ff`）、`MergeOutcome`。内容のマージは`git merge`と同じくhistogram diffで行い、競合マーカー（`merge.conflictStyle`の`merge`/`diff3`）の位置までGitと一致する。複数のmerge baseは仮想的な共通祖先にまとめる。コンフリクトはindexのstage 1〜3、`MERGE_HEAD`・`MERGE_MSG`・`MERGE_MODE`・`ORIG_HEAD`としてGitと同じ形で記録し、Gitで続行・中止できる。リネームは検出しない (#29)
- `create_commit`は、マージ中（`MERGE_HEAD`あり）なら`MERGE_HEAD`を第二親にしてマージ状態を片付ける（reflogは`commit (merge):`）
- stash: `Repository::stash_save()`（`git stash push`。`StashOptions`でメッセージと未追跡ファイルの対象化を指定）、`stash_list()`、`stash_apply()`（`--index`相当の指定あり）、`stash_pop()`、`stash_drop()`、`StashEntry`、`StashApplyOutcome`。Gitと同じ形式（`refs/stash`とそのreflog、index・未追跡ファイルのコミット）で保存し、Gitとzerogitのどちらで作ったstashも相互に扱える。適用は3-way mergeで行い、コンフリクトはmergeと同じ形で記録する（popはstashを残す）(#30)
- rebase: `Repository::rebase()`（`git rebase <upstream>`、`--onto`相当）、`rebase_continue()`、`rebase_skip()`、`rebase_abort()`、`is_rebasing()`、`RebaseOutcome`。Gitの既定（mergeバックエンド・非対話）と同じく、upstreamにないコミットを古い順に3-way mergeで付け替え、作者・メッセージを保つ。マージコミットは除外し、upstreamに同じ変更があるコミット（patch-id）は飛ばし、空になったコミットは捨てる。途中の状態を`.git/rebase-merge/`にGitと同じ形式で保存し、zerogit・Gitのどちらからでも再開・スキップ・中止できる。reflog（`rebase (start)`・`(pick)`・`(continue)`・`(finish)`・`(abort)`）と`ORIG_HEAD`もGitと同じ。対話的rebaseは範囲外 (#31)
- リモート設定: `Repository::remotes()`、`remote()`、`add_remote()`（既定のfetch refspec付き）、`set_remote_url()`、`remove_remote()`（設定・リモート追跡ブランチ・upstream設定を削除）、`branch_upstream()`、`set_branch_upstream()`、`Remote`、`Refspec`（`+`・`^`・`*`のパターン、対応付けと逆引き）(#32)
- packの受信と送信: `Repository::store_pack()`（受け取ったpackを検証し、deltaを解決してversion 2の`.idx`を作り`objects/pack/`に保存する。thin packは手元のオブジェクトで補う。`.idx`は`git index-pack`の出力と一致する）、`objects_to_send()`・`pack_objects()`（wantsから到達しhavesから到達しないオブジェクトのpackを作る。deltaなし）、`StoredPack` (#32)
- 新しいcrate `zerogit-remote`: `clone()`・`fetch()`・`push()`（と、トランスポートを指定する`*_with`）。Git protocol v2（v0へのフォールバックあり）でのfetch、receive-packでのpush。トランスポートはローカル（Gitを起動せず直接読み書き）、SSH（システムの`ssh`クライアント）、Smart HTTP(S)（`ureq`+`rustls`、Basic/Bearer認証）。clone・fetch・pushの結果（参照、設定、`FETCH_HEAD`、reflog、作業ツリー）がGitと一致することを、ローカル・`git upload-pack`/`receive-pack`・`git http-backend`の各経路で確認している (#32)
- 参照の操作: `Repository::references()`、`find_reference()`、`update_reference()`（期待値付きの更新・削除。packed参照も削除できる）、`set_symbolic_reference()`、`is_ancestor()`、`has_object()`、`object_type()`、`peel()`、`reset_hard()`、`is_bare()`
- `Error::StaleReference`
- `Config::get_all()`: 複数値のキー（`remote.<name>.fetch`など）の取得
- `Error::RemoteNotFound`、`RemoteAlreadyExists`
- `Error::RebaseInProgress`、`NoRebaseInProgress`、`UnsupportedRebase`
- `Error::MergeInProgress`、`NoMergeInProgress`、`NotFastForward`、`LocalChangesWouldBeOverwritten`、`UnsupportedMerge`
- 設定の読み込みで`GIT_CONFIG_NOSYSTEM`・`GIT_CONFIG_SYSTEM`・`GIT_CONFIG_GLOBAL`に従う
- symlinkをGitと同じくmode `120000`・リンク先パスを内容とするBlobとして扱う。`add`/`add_all`・status・作業ツリーとの差分・`checkout`に対応。リンク切れも`add`できる。`core.symlinks=false`や作成できない環境では、リンク先パスを内容とする通常ファイルとして書き出す (#22)

### Changed
- crates.ioのパッケージから`issues/`・`docs/`・`tests/`を除外した
- **破壊的変更**: `Error`に`UnmergedPaths`・`IgnoredPath`・`UnsupportedAttribute`・`IrreversibleLineEndings`・`MergeInProgress`・`NoMergeInProgress`・`NotFastForward`・`LocalChangesWouldBeOverwritten`・`UnsupportedMerge`・`RebaseInProgress`・`NoRebaseInProgress`・`UnsupportedRebase`・`RemoteNotFound`・`RemoteAlreadyExists`・`StaleReference`を追加した
- ブランチ名・タグ名を`git check-ref-format`の規則で検証する（空白、`@{`、`//`、末尾の`.`、`.`で始まる・`.lock`で終わる要素を追加で拒否。ブランチ名の`@`・`HEAD`も拒否）。`a`と`a/b`のように衝突する参照は`RefAlreadyExists`になる
- `Repository::open`が、`.git`を持たないbareリポジトリ（`core.bare=true`、または名前が`.git`で終わらないGitディレクトリ）をbareとして開く
- グローバル設定は`$XDG_CONFIG_HOME/git/config`の後に`~/.gitconfig`を読む（Gitと同じく`~/.gitconfig`が優先）
- 作業ツリーの走査で、名前が`.`で始まるファイル・ディレクトリ（`.github/`、`.env.example`など）を除外しない。除外するのは`.git`だけ (#20)
- `Repository::add`は、無視された未追跡ファイルを`IgnoredPath`で拒否する（`git add`と同じ）
- status・作業ツリーとの差分は、内容に加えてmode（実行ビット、symlink）の違いも変更として報告する。`core.fileMode=false`（およびUnix以外）ではindexのmodeを使う
- `checkout`は実行ビットを設定し、ファイルとsymlinkの置き換えに対応する
- indexに書くサイズを作業ツリー上のファイルサイズにし、時刻のナノ秒も記録する
- `Index::add`はstage 0の追加で同じパスの全stageを置き換え、`Index::remove`は全stageを削除する。エントリはGitと同じ順（パスのバイト列、次にstage）に保つ
- `Repository::add`は、作業ツリーから削除された追跡中のファイルの削除をステージする（`git add`と同じ）
- `Repository::add_all`は、HEADにないファイルを含め、作業ツリーにないindexのエントリを削除する（`git add -A`と同じ）

### Fixed
- 設定値の解析が、引用符の中のエスケープされた`"`や、値の途中の引用符・末尾の空白を正しく扱っていなかった
- 作業ツリーの走査がディレクトリへのsymlinkを辿らない（リポジトリ外の走査や無限ループを防ぐ）(#22)
- コンフリクト中のindexから`create_commit`すると、同名エントリが重複した不正なtreeを作っていた。`UnmergedPaths`を返し、何も書き込まない (#23)
- コンフリクト中の`checkout`を`UnmergedPaths`で拒否する (#23)
- `status()`がコンフリクト中のパスをstage 3の内容で比較していた。`Modified`として1件報告する (#23)
- treeのエントリを名前順に並べていたため、`foo`ディレクトリと`foo.txt`などが並ぶとGitの順序（ディレクトリは`foo/`として比較）と異なり、`git fsck`が不正と判定するtreeを作っていた

---

## [0.4.0] - 2026-10-04

### Added

#### Packfile・packed-refs読み取り
- pack v2/v3とidx v2（64bit offsetを含む）の読み取り。OFS_DELTA/REF_DELTAと多段deltaの復元に対応
- looseと複数packを横断して読み取る（read・存在確認・短縮OID検索）。外部の`git repack`/`git gc`にも追従
- `packed-refs`によるHEAD・ブランチ・リモートブランチ・タグの解決と一覧
- `objects::pack::{PackIndex, PackFile, PackLimits}`

#### 行単位差分
- `Repository::diff_blobs()`、`BlobDiff::compute()`: 旧新行番号付きの行差分（Myers法）
- `BlobDiff`、`BlobDiffContent`、`DiffHunk`、`DiffLine`、`DiffOptions`、`LineKind`、`LineEnding`、`NonTextReason`、`SkipReason`
- 利用例 `examples/document_diff.rs`、測定 `examples/measure_document_diff.rs`

#### 類似度リネーム検出（任意）
- `Repository::diff_trees_with_options()`、`Repository::commit_diff_with_options()`、`RenameOptions`、`RenameDetection`、`RenameLimit`
- `DiffDelta::similarity()`（完全一致は100）、`TreeDiff::rename_limits()`

#### エラー
- `InvalidPackedRefs`、`PackedRefDeletionUnsupported`、`InvalidPackIndex`、`UnsupportedPackIndexVersion`、`InvalidPack`、`UnsupportedPackVersion`、`UnsupportedRepositoryFormat`、`UnsupportedIndex`、`PackLimitExceeded`
- `IndexEntry::ctime_nsec()`、`mtime_nsec()`、`skip_worktree()`、`intent_to_add()`

### Changed
- **破壊的変更**: `Error`にvariantを9つ追加した（上記「エラー」）。`Error`を網羅的に`match`しているコードは、新しいvariantへの対応が必要
- `Repository::open`/`discover`/`init`は`.git/config`を読み、SHA-256（`extensions.objectFormat`）・reftable・未知の`repositoryformatversion`を`UnsupportedRepositoryFormat`として拒否する。configが壊れている場合もopen時にエラーになる
- packed参照を含むブランチの削除は`PackedRefDeletionUnsupported`を返す

### Fixed
- 完全一致リネームで、移動と同時に実行権限が変わった場合も旧新modeを正しく保持する。同じ内容のファイルが複数ある場合の対応付けを決定的・一対一にした
- 製品コードでI/Oエラーや破損を「オブジェクトなし」として扱っていた箇所を、明示的なエラーにした
- index v4（パス圧縮）を正しく読み書きする。以前はv4を誤解析し、書き戻すとGitが読めないindexになっていた（#16）。読んだバージョンで書き戻すため、ヘッダとエントリの形式は常に一致する
- index読み取りでチェックサム・パディング・名前の終端・拡張フラグを検証する。nanosecond時刻とskip-worktree・intent-to-addのフラグを書き戻しで保持する
- split index・sparse indexは`Error::UnsupportedIndex`として明示的に拒否する（誤読・破損書き込みを防止）。skip-worktreeを含むindexの全体reset・checkoutも同様に拒否する
- intent-to-add（`git add -N`）のエントリをコミットに含めない。skip-worktreeのエントリをstatus・作業ツリー差分で削除扱いにしない
- reset・checkoutでindexを作り直す際に、実行権限・symlinkのmodeを失っていた問題を修正

---

## [0.3.7] - 2026-01-20

### Added

#### Git Config読み取り
- `Repository::config()`: リポジトリの設定を取得
- `Config::get()`: 設定値をキーで取得（`user.name`, `user.email`など）
- ローカル設定（`.git/config`）の読み取りに対応

---

## [0.3.6] - 2026-01-20

### Fixed

#### ログのパスフィルタリング改善
- `log_with_options()`のパスフィルタリングがサブディレクトリ内のファイルを正しく検出するように修正
- ディレクトリプレフィックス指定（`src/`や`src`）で配下の全ファイルの変更を検出可能に
- ネストされたパス（`src/utils/helpers/mod.rs`など）のフィルタリングに対応

---

## [0.3.5] - 2026-01-20

### Added

#### リポジトリ初期化
- `Repository::init()`: 新規Gitリポジトリを初期化
- 必要なディレクトリ構造（`.git/objects`, `.git/refs/heads`, `.git/refs/tags`）を自動作成
- デフォルトブランチは `main`

#### Commit OID取得
- `Commit::oid()`: コミット自身のOIDを取得するメソッドを追加
- `Oid::short()`: 短縮形式（7文字）のOIDを取得

#### ローカルブランチ一覧
- `Repository::branches()`: ローカルブランチ一覧を`Vec<Branch>`として取得
- `remote_branches()`と対称的なAPIを提供

### Fixed
- `Repository::log()` で各コミットのOIDが取得可能に

---

## [0.3.0] - 2026-01-20

### Added

Phase 2.5: 参照拡張・ログフィルタリング・差分機能の完全実装。

#### リモートブランチ・タグ対応
- `Repository::remote_branches()`: リモートブランチ（refs/remotes/*）の一覧取得
- `Repository::tags()`: タグ（refs/tags/*）の一覧取得
- `RemoteBranch` 型: リモート名とブランチ名を分離して取得可能
- `Tag` 型: 軽量タグ・注釈付きタグ両対応、メッセージ・tagger情報取得可能
- 注釈付きタグオブジェクト（tag object）のパース対応

#### ログフィルタリング
- `Repository::log_with_options()`: フィルタリング付きログ取得
- `LogOptions` ビルダー: 柔軟なオプション指定
  - `path()` / `paths()`: 特定ファイル・ディレクトリの変更履歴
  - `max_count()`: 最大取得件数
  - `since()` / `until()`: 日付範囲フィルタ
  - `first_parent()`: マージの片側のみを辿る
  - `author()`: 作者名でフィルタ
  - `from()`: 開始コミット指定

#### Tree Diff
- `Repository::diff_trees()`: 2つのTree間の差分計算
- `TreeDiff` 型: 差分結果のコンテナ
- `DiffDelta` 型: 各変更エントリ（パス、ステータス、OID）
- `DiffStatus` enum: Added, Deleted, Modified, Renamed, Copied
- `DiffStats` 型: 変更ファイル数の統計
- 完全一致リネーム検出対応

#### コミット変更一覧
- `Repository::commit_diff()`: コミットの変更ファイル一覧取得
- 初期コミット（親なし）対応
- マージコミット対応（最初の親との差分）

#### ワーキングツリー・Index差分
- `Repository::diff_index_to_workdir()`: git diff 相当
- `Repository::diff_head_to_index()`: git diff --staged 相当
- `Repository::diff_head_to_workdir()`: git diff HEAD 相当

### Changed
- `LogIterator` 内部構造をフィルタリング対応に拡張

---

## [0.2.0] - 2026-01-18

### Added

Phase 2: 書き込み操作の完全実装。

#### ステージング操作
- `Repository::add()`: ファイルをステージングエリアに追加
- `Repository::add_all()`: 全変更（新規、変更、削除）をステージ
- `Repository::reset()`: ステージを解除（HEADの状態に戻す）

#### コミット作成
- `Repository::create_commit()`: インデックスからコミットを作成
- ツリーオブジェクトの自動構築（サブディレクトリ対応）
- 親コミットの自動検出とHEAD更新

#### ブランチ操作
- `Repository::create_branch()`: 新しいブランチを作成
- `Repository::delete_branch()`: ブランチを削除（現在のブランチは削除不可）
- `Repository::checkout()`: ブランチまたはコミットに切り替え
- ネストされたブランチ名のサポート（例: `feature/foo`）
- detached HEAD状態への切り替え対応

#### インデックス書き込み
- `Index::write()`: インデックスをファイルに書き込み
- `Index::add()`: エントリを追加/更新
- `Index::remove()`: エントリを削除
- `Index::empty()`: 空のインデックスを作成
- チェックサム計算とv2形式での出力

#### オブジェクト書き込み
- `LooseObjectStore::write()`: looseオブジェクトの書き込み
- `compress()`: zlibフォーマットでの圧縮
- 冪等性の保証（既存オブジェクトは再書き込みしない）

### Changed
- `Index` 構造体に可変操作メソッドを追加

---

## [0.1.0] - 2026-01-17

### Added

Phase 1: Repository Layer（読み取り操作）の完全実装。

#### オブジェクト操作
- `Oid`: SHA-1オブジェクトID型（16進文字列変換、短縮形式対応）
- `Blob`: blobオブジェクト（ファイル内容）の読み取り
- `Tree`: treeオブジェクト（ディレクトリ構造）の読み取り
- `Commit`: commitオブジェクト（コミット情報）の読み取り
- `LooseObjectStore`: loose objectの読み取りと前方一致検索

#### 参照解決
- `RefStore`: 参照ファイルの読み取りとシンボリック参照の解決
- `Head`: HEAD参照（ブランチまたはdetached HEAD）
- `Branch`: ブランチ情報と一覧取得
- タグ一覧の取得

#### リポジトリ操作
- `Repository::open()`: 指定パスでリポジトリを開く
- `Repository::discover()`: 親ディレクトリを探索してリポジトリを発見
- `Repository::commit()`: 短縮SHA-1でコミットを取得
- `Repository::tree()`: ツリーオブジェクトを取得
- `Repository::blob()`: blobオブジェクトを取得
- `Repository::head()`: HEAD参照を取得
- `Repository::branches()`: ブランチ一覧を取得
- `Repository::log()`: コミット履歴をイテレート
- `Repository::status()`: ワーキングツリーの状態を取得

#### インデックス
- Git index（.git/index）のパース（v2/v3/v4対応）
- インデックスエントリの読み取り

#### ステータス
- Untracked files（未追跡ファイル）の検出
- Modified files（変更ファイル）の検出
- Deleted files（削除ファイル）の検出
- Staged changes（ステージされた変更）の検出

#### インフラストラクチャ
- Pure Rust SHA-1実装
- zlib解凍（miniz_oxide使用）
- ファイルシステムユーティリティ

### Dependencies
- `miniz_oxide` 0.8 - zlib解凍

### Notes
- 最小Rustバージョン: 1.70.0
- 対応プラットフォーム: Linux, macOS, Windows
- テストカバレッジ: 94%以上

[0.8.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.8.0
[0.7.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.7.0
[0.6.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.6.0
[0.5.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.5.0
[0.4.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.4.0
[0.3.7]: https://github.com/siska-tech/zerogit/releases/tag/v0.3.7
[0.3.6]: https://github.com/siska-tech/zerogit/releases/tag/v0.3.6
[0.3.5]: https://github.com/siska-tech/zerogit/releases/tag/v0.3.5
[0.3.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.3.0
[0.2.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.2.0
[0.1.0]: https://github.com/siska-tech/zerogit/releases/tag/v0.1.0
