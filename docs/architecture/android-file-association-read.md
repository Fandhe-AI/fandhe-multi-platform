# Android のファイルの関連付け: 止まる読み取りを回収する設計

- 対応: Issue #451（親 #102）。未マージの実装は PR #436（ブランチ `feat/418-android-file-association`）
- 契約の正: `crates/fandhe-device/src/file_association.rs`（本書は Android 固有の決定だけを書く）
- 状態: 設計のみ。実装が済むまで対応表の Android セルの `deferred` は外さない

## 1. 背景

PR #436 は、届いた `content:` URI を本体プロセスの worker スレッドで開いて読んだ。送り手の ContentProvider が
`StorageManager.openProxyFileDescriptor` で fd を返して `onRead` を止めると、`Os.poll` が読み取り可能を返しても
`read` が止まる。worker・fd・一時ファイルが回収できず、4 回で worker が埋まり、以降は正常なファイルも拒否された。

## 2. 決定の要約

起案は「`android:process` の Service で読み、pipe で本体へ渡し、期限切れは `Process.killProcess` で回収する」だった。
別プロセスで読む点は採用する。次の 3 点は、裏取りの結果を受けて変えた。

| 起案 | 決定 | 変えた理由 |
| --- | --- | --- |
| 止まった読み取りは kill で回収できる | **proxy fd（appfuse）は読まずに失敗にする**。kill はそれ以外の止まり方の回収に使う | proxy fd の read で止まったプロセスは kill しても死なない（§3 の V1） |
| pipe で中身を本体へ渡す | **pipe を使わない**。読み手が名前・サイズ・先頭 16 バイトだけを binder で返す | 契約が保持するのはこの 3 つだけ。8 MiB を本体へ運ぶ必要がない |
| kill 完了まで次の要求を受けない | **順番待ちにする**。ただし kill が完了しないときは待たせ続けず失敗にする | kill の完了に上限が無い（V1）。後ろに並んだ正常なファイルを巻き添えにしない |

## 3. 裏取りの結果

実測は Android 16（API 36）の arm64 エミュレータ 1 種で、UID の異なる送り手・受け手の検証アプリを使った。実機と他の API レベルは未確認。

### 確認済み

| # | 問い | 結果 | 根拠 |
| --- | --- | --- | --- |
| V1 | proxy fd の read で止まった読み手を kill で回収できるか | **できない**。`killProcess` 後、主スレッドは zombie、read 中のスレッドは D 状態のまま残り、`SIGKILL` は保留のままだった。binder death は届かず、`ActivityManager` はプロセスを生存扱いのまま。送り手を終了して初めて回収された | 実測。原因は Linux の FUSE で、デーモンへ渡した要求の応答は `wait_event` で無条件に待つ（[fs/fuse/dev.c の `request_wait_answer`](https://github.com/torvalds/linux/blob/master/fs/fuse/dev.c)） |
| V2 | proxy fd を事前に見分けられるか | 実測の範囲ではできる。`/proc/self/fd/<n>` の読み先が `/mnt/appfuse/` で始まる。読み先の取得で送り手は呼ばれない | 実測。**公開 API ではない**ので V10 と §5.4 の後ろ盾を置く |
| V3 | proxy fd を読まずに閉じれば止まらないか | 止まらない。`onRead` の停止中も、コールバック用 Handler の停止中も `close` は戻った | 実測 |
| V4 | proxy fd 以外の止まり方は kill で回収できるか | できる。`query`・`openFile` の binder 待ち、書き手が閉じない pipe の read のいずれも、binder death がすぐ届いた | 実測 |
| V5 | 別プロセスへ URI 許可は届くか | 届く。受け手の Activity へ渡した `FLAG_GRANT_READ_URI_PERMISSION` だけで、同じ UID の別プロセスから転送なしで `getType`・`query`・`openFileDescriptor` が通った | 実測。実装上の単位は UID（[`UriGrantsManagerService` の `targetUid`](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/services/core/java/com/android/server/uri/UriGrantsManagerService.java)）。公式文書は UID 単位と明記していない |
| V6 | 許可はいつまで続くか | 受信した Activity の終了まで。`finish` 後に開くと `SecurityException` になった。送り手の force-stop でも失われた | [公式文書](https://developer.android.com/guide/topics/providers/content-provider-basics)「last until the activity that receives them is finished」と実測 |
| V7 | `CancellationSignal` で開く段を中断できるか | できるとは限らない。`openFile` を止めた送り手では `cancel()` が戻っても `openFileDescriptor` は戻らなかった。信号を受けるかは送り手の実装次第 | 実測と [`ContentResolver`](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/content/ContentResolver.java)（信号は送り手へ転送するだけ） |
| V8 | 読み手を kill した後に新しい読み手を使えるか | 使える。ただし接続を残すと、システムが Service を自動で再起動する。binder death で `unbindService` してから次に bind すれば、新しいプロセスで次の要求を処理できた | 実測と [`ServiceConnection`](https://developer.android.com/reference/android/content/ServiceConnection)（「this binding to the service will remain active」） |
| V9 | 送り手が読み取り中に死ぬとどうなるか | 読み手も死に、本体は生き残った。開いた fd は送り手への stable 参照を持ち、送り手が死ぬと依存先として kill される。この実装のとおりなら、本体で読むと本体ごと死ぬ（本体で読む形は未測定）。別プロセスにする理由の 1 つ | 実測と [`ContentProviderHelper`](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/services/core/java/com/android/server/am/ContentProviderHelper.java)（`depends on provider`） |

`killProcess` で同じアプリの別プロセスを kill できることは[公式文書](https://developer.android.com/reference/android/os/Process#killProcess(int))に
書かれている。`getType` を止めた送り手では、この環境だと `getType` が打ち切られて `null` を返した。打ち切りの有無には依存せず、§5.3 の期限で扱う。

### 未確認（実装の最初に確かめる）

| # | 確かめること | 外れたときの扱い |
| --- | --- | --- |
| V10 | minSdk 24 から現行までと実機で、proxy fd の読み先が `/mnt/appfuse/` で始まること（API 26 未満は API 自体が無い） | 判定を見直す。すり抜けても §5.4 の範囲で止まる |
| V11 | Files アプリなど通常の送り手の `.txt` が proxy fd でなく読めること | 通常の送り手が proxy fd を返すなら、本設計の範囲では読めない。機能の価値を判断し直す |
| V12 | 開いた後に受信 Activity が終了しても読み終えられるか | どちらでも嘘の成功にはならない。結果に合わせて §7 の記述を直す |
| V13 | 本体が死んだときに読み手の `onUnbind` が呼ばれること（§5.5） | 別の手段で読み手を止める |
| V14 | 画面の回転など Activity の再生成で許可が失われないこと | 失われるなら §7 に書き、再生成中の要求は失敗として扱う |

## 4. 構成

- **読み手**: `android:process=":fileread"`、`android:exported="false"` の bound Service（[`android:process`](https://developer.android.com/guide/topics/manifest/service-element#proc)）。
  Java だけで書き、native ライブラリを読み込まない。URI 許可が UID に付くので `isolatedProcess` にしない
- **本体**: 受け付けと状態の管理を main looper の直列キュー 1 本に集約する（`device-bridge-contracts.md` 類型 11）。
  binder death などの通知もここへ post する。処理中に本体が止まる呼び出しはしない
- **bind**: Activity の再生成で切れないよう `ApplicationContext` から行う。キューが空になったら `unbindService` する。
  読み手は読み取り中だけ必要なので、待機中のメモリを増やさない

1 件の流れは次のとおり。要求 id を付け、現在の要求でない応答は捨てる。

1. `onCreate` / `onNewIntent` の `ACTION_VIEW` をキューに積む（`file:` は読まずに拒否として積む）
2. 順番が来たら期限を始め、未接続なら bind する。読み手は接続直後に自分の pid を返す
3. 読み手が `getType` と `query` を行い、名前と MIME を本体へ返す
4. 本体が `nativeAccepts`（共通層の `accepts`）で判定する。通らなければ拒否を確定する
5. 読み手が開いて読み、結果を返す。本体が結果を確定して次へ進む

判定を Java に複製しないために手順 3〜4 で往復する。読み取りの成否より先に拒否を確定するのは PR #436 の指摘への対応で、
`FileInbox` の「拒否は `latest` を変えない」を守るため。

## 5. 期限と kill

### 5.1 kill の完了の観測

完了は `IBinder.linkToDeath` の `binderDied` だけで判定する（[`DeathRecipient`](https://developer.android.com/reference/android/os/IBinder.DeathRecipient)）。
時間の経過で完了とみなさない。binder スレッドで呼ばれるので main looper へ post し、最初に `unbindService` する（V8）。
`onServiceDisconnected` は実測では同時に届いたが、判定には使わない。観測点を 1 つにして二重処理を避ける。

kill の対象は、binder death をまだ受けていない読み手が返した pid だけにする。死を観測していない間は pid が再利用されない。

### 5.2 状態

| 状態 | 意味 | 新しい要求 |
| --- | --- | --- |
| `IDLE` | 読み手なし、または待機中 | すぐ処理する |
| `BUSY` | bind 中または 1 件を処理中 | 順番待ち |
| `KILLING` | kill 済みで `binderDied` 待ち | 順番待ち |
| `UNRECLAIMED` | `KILLING` のまま 3 秒経過 | 待たせず失敗 |

`binderDied` が届くと `KILLING`・`UNRECLAIMED` は `IDLE` へ戻り、次の要求を新しい読み手で処理する。

### 5.3 kill 中に届いた要求は待機させる

拒否にすると、止まった送り手の後ろに並んだ正常なファイルまで失敗になる。そこで順番待ちのままにする。
ただし kill の完了に上限は無い（V1）ので、`KILLING` が 3 秒続いたら待機中の要求をすべて失敗に確定し、`UNRECLAIMED` へ移る。

3 秒は「後続を待たせるのをやめる」時間で、kill の完了を意味しない。`UNRECLAIMED` でも解放の記録は消さず、次の読み手を起動しない（類型 9）。
定数なので、実機の結果で調整してよい。

- 1 件の期限は 10 秒（PR #436 と同じ）。起点は順番が来て処理を始めた時点で、単調時計（`SystemClock.uptimeMillis`）で測る
- 期限切れは、状態を `KILLING` にしてから失敗を確定し、kill する。pid を受け取る前の期限切れは kill せず、`unbindService` して失敗を確定する
- 待ち行列は 32 件まで（PR #436 と同じ）。溢れた分の失敗も同じキューの順番で確定し、先行要求より先に結果を出さない

### 5.4 proxy fd は読まない

開いた直後、fd に対する他の操作より先に `Os.readlink("/proc/self/fd/<n>")` を見る。`/mnt/appfuse/` で始まれば読まずに閉じて失敗にする。
`fstat` も FUSE へ届くので、この判定より前に呼ばない。

判定は公開 API ではない。将来すり抜けた場合は、kill が完了せず `UNRECLAIMED` に留まる。読み手 1 プロセスが残り、以後の要求は失敗し続けるが、
偽の成功は出ず、資源もそれ以上は増えない。送り手が応答するか終了すれば `binderDied` で復帰する。

### 5.5 本体が先に死んだとき

読み手は処理中に `onUnbind` が呼ばれたら、呼び出し元が消えたとみなして自分を kill する。止まった読み取りをキャッシュ済みプロセスとして残さないため（V13）。
本体は処理中に `unbindService` しない（kill は常に先に行う）ので、通常の経路とは衝突しない。

## 6. 開く段と読み取りの契約

- **開く段の中断**: `CancellationSignal` を渡さず、3 引数ではなく 2 引数の `openFileDescriptor(uri, "r")` を使う。効くかが送り手次第で（V7）、
  止まれば結局 kill が要る（V4）。中断手段を 1 つにする
- **上限**: `MAX_FILE_BYTES`（8 MiB）ちょうどまで受理し、超えた時点で読むのをやめて `TooLarge` にする
- **先頭**: `HEAD_BYTES`（16 バイト）だけを返す
- **定数の正**: どちらも Rust 側。本体が JNI で読み、要求に載せて読み手へ渡す。読み手は native を読み込まず、Java に複製もしない（類型 5）
- **サイズ**: `st_size` や `_size` を信じず、実際に読んだバイト数を使う。pipe では `st_size` が 0 になる
- **0 バイト**: 他 OS の `read_picked_from` と同じく成功
- **途中切れ**: EOF で `ParcelFileDescriptor.checkError()` を呼ぶ。`closeWithError` の途中切れを成功にしない（PR #436 の指摘）
- **名前**: `DISPLAY_NAME` だけを使い、取れなければ拒否する。`MAX_NAME_BYTES` を超える名前は binder に載せず、読み手の側で拒否として返す
- **一時ファイル**: 作らない。固定長のバッファでサイズを数えて先頭だけ残すので、回収するものが無い
- **Rust 側**: 開いた `File` を受ける `receive_file` は使わない。名前・サイズ・先頭から `latest` を更新する入口を `FileInbox` に足す。
  更新後に `crate::android::request_repaint` で次フレームを起こす（類型 4）
- **ログ**: URI・名前・例外メッセージ・中身を出さない

## 7. 結果の写像

| 場面 | 結果 |
| --- | --- |
| EOF まで読めて `checkError` も通った | 受理（名前・サイズ・先頭 16 バイト） |
| 名前が取れない・`accepts` を通らない・`file:` | 拒否（`rejected` だけ増え `latest` は変わらない） |
| 8 MiB を超えた | `TooLarge` |
| `getType` がフォルダの MIME、または fd がディレクトリ | `Folder` |
| 期限切れ（どの段でも） | `Unreadable` |
| 読み手プロセスの死（kill・送り手の死に巻き込まれた場合・その他） | `Unreadable` |
| proxy fd | `Unreadable` |
| 開く段の例外・`null`（許可の喪失 `SecurityException` を含む）、読み取りの例外、`checkError` の失敗 | `Unreadable` |
| 読み手に接続できない、待ち行列の溢れ、`UNRECLAIMED` | `Unreadable` |

**3 値について**: この機能に利用者のキャンセル操作は無い。OS が渡したファイルを読むだけで、取り消す画面が無いため。
そのためキャンセルは結果に持たず、成功（受理）か失敗の 2 つになる。Activity の終了による許可の喪失も、読めなかった事実として失敗にする。
`CancellationSignal` を渡さないので `OperationCanceledException` も起きない。

既存の `OpenFailure` の 3 種で足りるので、契約の種類は増やさない。

## 8. 守る範囲と守らない範囲

守る:

- 1 プロセス内の受信順（失敗の通知を含む）
- 止まる送り手に対して、fd・スレッド・プロセスを回収し、後続の要求を処理できること（proxy fd は読まないので止まらない）
- 失敗を成功と表示しないこと。途中切れを受理にしないこと
- 名前・中身をログへ出さないこと

守らない（資源の漏れ・データ消失・嘘の成功を伴わないもの）:

- proxy fd を返す送り手のファイルは読めない。`Unreadable` になる。[公式文書](https://developer.android.com/reference/android/os/storage/StorageManager#openProxyFileDescriptor(int,%20android.os.ProxyFileDescriptorCallback,%20android.os.Handler))が
  用途に挙げるクラウド上のファイルなどが該当しうる
- 受信 Activity の終了後の読み取り（V6、`Unreadable`。元のファイルは送り手に残る）
- 本体プロセスが死んだ後の再表示と、待ち行列にあった要求
- 複数プロセスからの受信（受け口は本体の 1 プロセスだけ）

範囲外にできないため明記する制約: 判定をすり抜けた未知の FUSE で読み取りが止まると、アプリからは回収できない（V1。読み手 1 プロセスが残る）。
§5.4 のとおり 1 プロセスに抑えて後続を失敗にする。V10 でこの経路が現実に通ると分かったら、実装を進めず方針を見直す。

## 9. 採らなかった案

- **本体で poll + 非ブロッキング読み取り（PR #436 の方式）**: proxy fd では poll が読み取り可能を返して read が止まる。binder 待ちも本体のスレッドでは中断できない
- **読み手を複数のプロセスに分ける**: 止まったプロセスを回収できない問題は同じで、枠が埋まる時期を遅らせるだけ
- **proxy fd を受け入れ、止まったら `UNRECLAIMED` で耐える**: 1 つの送り手が止まるだけで、その送り手が終了するまで全ファイルが読めなくなる。#451 の完了条件を満たさない
- **既知の送り手だけ受ける**: 関連付けの受け口は任意のアプリから届くため、機能が成り立たない。proxy fd の判定で足りる
- **Service の `startService` に許可を付けて寿命を延ばす**: 実測では Activity の終了後も読めた。ただし kill で開始記録が消えると、後ろに並んだ要求の許可まで失う恐れがあり、失敗の経路が増える。利点が小さいので採らない

## 10. 実装の順序

1. §3 の未確認（V10〜V14）を確かめ、外れがあれば本書を直す
2. `FileInbox` の入口と JNI、読み手の Service、本体のキューを実装する。PR #436 の manifest・intent-filter・`MainActivity` の分岐は流用できる
3. 止まる送り手のテスト用アプリで、proxy fd・pipe・`query`・`openFile` の各場面を流し、期限後に後続の `.txt` が読めることを確かめる
4. `file-association.json` の `deferred` を外して `done` にする（#451 の完了条件）
