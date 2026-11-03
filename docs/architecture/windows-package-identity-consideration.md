# Windows パッケージ ID 導入の検討記録（ADR）

- **状態: 提案（未決定）**。導入の是非はオーナーが判断します。本書を入れても導入したことにはなりません
- 対象: 端末機能の Windows 実装（`crates/fandhe-egui-proxy/src/device/windows_*.rs`）と対応表の Windows セル
- 関連 Issue: #3（ストア提出パイプライン。Windows `.msix` を含む）、#335（対応表の `no` の振り分け）
- 本書で変えないもの: コード、マニフェスト、CI、対応表 JSON（`docs/device/capabilities/*.json`）
- 実機確認: **本書の内容は Windows 実機で 1 つも試していません**。公式文書と対応表の読み取りだけで書いています。
  「未確認」と書いた点は確定事項として扱わないでください

## 1. 結論の要約

- identity だけで外れる見込みのセルは **6 件**で、起案時の見立て（`background-upload`・`file-provider` の 2 件）より多い
- 自己署名は手元の確認専用です。公開配布には使えないため、配布の経路は別に決める必要があります
- 推し案は「sparse package + 自己署名を、既定のビルドを変えない opt-in の確認経路として採る。ただし素の exe の
  Windows 実機確認を先に済ませる」です（§6）

## 2. 文脈

### 2.1 パッケージ ID と現状

Windows の一部の機能は、実行時にパッケージ ID（package identity、以下 identity）を持つプロセスだけが使えます。
本リポジトリの Windows 版は素の exe（unpackaged）で、次の状態です。

- `.github/workflows/` に Windows のジョブは無く、`platforms/` にも Windows のプロジェクトは無い
- exe へ manifest を埋め込む処理は無い
- 対応表の Windows セルは 164 件で、内訳は `done` 109、`no` 38、`possible` 11、`n-a` 6
- `done` 109 件のうち `verified: true` は **0 件**。素の exe で動くこと自体がまだ確かめられていない

### 2.2 identity を理由に止めているセルの実測

Windows セルの `api`・`perm`・`note`・`detail`・`deferred`・`approval` を identity 関連の語（identity、MSIX、パッケージ、
unpackaged、素の exe、manifest、capability 等）で検索すると、`done` 以外で 23 件（`no` 14・`possible` 9）が該当します。
全件を読むと、identity が理由に含まれるものは **18 件**、語が出るだけで理由が別のものは 5 件でした。

- 理由が別の 5 件: `ar`・`ar-camera-view`（Windows Mixed Reality の削除）、`map-view`（UWP 専用・基盤の廃止予定）、
  `spotlight-index`（相当する API が無い）、`picture-in-picture`（Windows App SDK の導入が前提）

18 件は、identity を得た後に何が残るかで次の 4 区分に分けました。区分は文書の読み取りによる見込みです。

#### 区分 A: identity で外れる見込み（6 件）

公式文書が identity を要件に挙げ、申請・有料登録・追加機材が要らないもの。

| セル | 現状 | priority | 根拠 | 残る確認 |
| --- | --- | --- | --- | --- |
| `background-upload` | `possible`（`deferred`） | 127 | 「Background transfers require package identity」 | sparse で `BackgroundUploader` が動くか |
| `share-receive` | `no` | 87 | identity で使える機能に Share target が挙がり、sparse の手順にも共有先の登録の記述がある | manifest の拡張と起動経路の新規実装 |
| `deep-link` | `no` | 118 | `windows.appUriHandler` は「full MSIX package or a sparse package」と明記 | 本番は自分のドメインへ JSON を置く。文書は対応ブラウザではブラウザ内に留まるとも書いており、効く範囲は限られる |
| `device-identifier` | `no` | 138 | `GetSystemIdForPublisher` は発行元ごとに値が分かれると明記 | 素の exe で全アプリ同値という記述はセルの note によるもので、今回読んだ頁には無い。Publisher を変えると値が変わる見込み |
| `file-provider` | `possible`（`deferred`） | 151 | 同期エンジンは Desktop Bridge を実装要件とする記述 | sparse が要件を満たすか。素の exe で登録できるかも未確認で、できるなら identity は不要 |
| `app-intents` | `no` | 103 | 「Apps must have package identity in order to register an app action」 | 定義 JSON はパッケージ内の内容として書かれており、sparse の置き場は未確認 |

#### 区分 B: identity を得ても未確認が大きいもの（5 件）

| セル | 現状 | priority | 残る点 |
| --- | --- | --- | --- |
| `home-widget` | `possible`（`deferred` なし、note に保留の記載） | 25 | 文書は「packaged Win32 desktop app」と書き sparse に触れない。Windows App SDK も要る |
| `calendar` | `no` | 31 | `appointmentsSystem` は restricted capability。sideload に承認は不要だが、Store では原則承認されないと明記 |
| `contacts` | `no` | 30 | `contactsSystem` も同じ扱い |
| `call-ui` | `no` | 37 | `voipCall` が要る。identity 必須はセルの note でも推定で、一次情報の明記が無い |
| `per-app-language` | `no` | 102 | `ApplicationLanguages` は公式の identity 必須一覧にある。OS 側のアプリ別設定にどう対応するかは未確認 |

#### 区分 C: identity を得ても別の条件が残るもの（5 件）

| セル | 現状 | priority | 残る条件 |
| --- | --- | --- | --- |
| `in-app-purchase` | `possible`（`approval`） | 60 | Partner Center への提出と Store の認証 |
| `in-app-review` | `possible`（`deferred`） | 83 | Store の製品との関連付け |
| `on-device-llm` | `possible`（`approval`） | 53 | Limited Access Feature の申請、Windows App SDK、対応機種 |
| `local-peer-connect` | `possible`（`deferred`） | 75 | Wi-Fi Direct 対応 PC 2 台。素の exe での可否も未確認 |
| `nfc` | `possible`（`deferred`） | 156 | NFC を持つ確認機材。素の exe での可否も未確認 |

#### 区分 D: identity を得ても外れないもの（2 件）

`cloud-kv-sync`・`cloud-db-sync`。roaming データが Windows 11 で非サポートのため、identity の有無では変わりません。

### 2.3 起案時の見立ての検証

| 見立て | 検証結果 |
| --- | --- |
| identity で外れるのは `background-upload`・`file-provider` の 2 件だけ | **崩れた**。`deferred` のうち identity だけが理由のものに限れば 2 件で正しいが、`no` のセルと note に理由を書いたセルを見落としていた。区分 A は 6 件、区分 B まで含めると最大 11 件 |
| 方式は sparse package + 自己署名 + exe への manifest 埋め込み | **確認用としては成り立つ**。手順は公式の手順どおり。ただし公式は自己署名を開発・試験用とし、公開配布には使えないと明記（§4） |

区分 A の priority は 87〜151 で、着手順としては後ろ寄りです。区分 B には `home-widget`（25）など前寄りのものがありますが、
sparse で足りるかが未確認です。

## 3. 既存 `done` セルへの影響

identity を付けると、次の挙動が変わる可能性があります。いずれも**未確認**です。Windows の `done` は実機確認が 0 件のため、
先に素の exe の結果を記録しないと、identity の影響かどうかを切り分けられません。

| セル | 今の作り | 変わりうる点 | 確認に要る手順 |
| --- | --- | --- | --- |
| `notification` | `.lnk` に固有 AUMID を付け、`CreateToastNotifierWithId` で送る | プロセスの AUMID がパッケージ由来になる見込みで、固有 AUMID との関係は公式の記述を見つけられなかった。送信元の名前・画像は manifest 由来になりうる | identity の有無で送信し、送信元の表示、通知設定の項目数、既存 `.lnk` の扱いを見る |
| `alarm` | 同じ AUMID で `ScheduledToastNotification` を予約 | 切り替え前の予約を一覧できず、取り消せなくなる可能性 | 素の exe で予約後に identity 付きで起動し、一覧と取り消しを見る |
| `recent-documents` | 明示 AUMID があれば `SetAppID` で渡す | 文書は明示 AUMID を全箇所で揃えるよう求めており、登録先と読み戻し先がずれる可能性 | `Listed` / `NotListed` を identity の有無で比べる |
| `app-shortcuts` | AUMID を明示しない | ジャンプリストの出方が変わる可能性（明示 AUMID との干渉は既知の天井） | ジャンプリストの表示と選択時の起動を見る |
| `camera-preview-stream`・`video-capture`・`mic-record`・`speech-recognition` | 宣言なしで、設定の「デスクトップ アプリ」の許可に従う | 公式は identity で専用の切り替えと同意プロンプトが付くと記述。公式の manifest 例に `webcam` / `microphone` は無く、full trust で宣言が要るかは読み取れなかった。要るのに宣言しないと `E_ACCESSDENIED` になり、既存の写像で権限拒否と表示される | 宣言の有無 × 許可・拒否で確かめる。プロンプトの有無、設定画面の項目、MTA のワーカーでの初期化が通るかも見る |
| `camera-capture` | ダミー実装 | なし（実カメラに未接続） | 不要 |
| `location` | PowerShell の子プロセス経由 | 子プロセスが identity を持つかは未確認で、同意の主体が exe にならない可能性 | 子プロセスのまま許可・拒否を確かめる |
| `human-presence`・`radio-control`・`activity-recognition`・`ble-peripheral` | 素の exe では宣言不要の見込み | セルの記述はパッケージ時に capability（`humanPresence`・`radios`・`activity`・`bluetooth`）が要るとする | identity 付きで宣言の有無を比べる |
| `launch-at-login`・`url-scheme`・`file-association` | HKCU へ直接書く | 公式の仮想化の記述は virtualized app 向けで、sparse の例は `unvirtualizedResources` を宣言するため変わらない見込み | 書いた値を `regedit` で見て、実際に起動を確かめる |
| `device-info` | `RtlGetVersion` と HKLM の読み取り | manifest の影響を受ける `GetVersionEx` を使っていないため変わらない見込み | 埋め込み前後で表示を比べる |
| `open-system-settings` | パッケージ ファミリー名（PFN）が無いので自アプリの頁は非対応と返す | identity があると PFN があり、非対応の返答が実情と合わなくなる | identity の有無を見て返すかを決め、頁が開くかを見る |
| `secure-credential` | `PasswordVault` を使う | 切り替え前の保存値を読めるかの記述を見つけられなかった。読めないとデータの消失に当たる | 素の exe で保存し、identity 付きで読み戻す |

このほか、公式の [identity 必須一覧](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/winrt-api-desktop-app-support)
（318 クラス、文書は網羅ではないと注記）に載るクラスを、18 個の `windows_*.rs` が使っています。

- 例: `pdf_viewer`（`PdfDocument`）、`share`（`DataTransferManager`）、`speech_recognition`（`SpeechRecognizer`）、
  `alarm`（`ScheduledToastNotification`）、`flashlight`（`Lamp`）、`image_recognition`（`OcrEngine`）
- `InMemoryRandomAccessStream`・`DataReader` のような汎用のクラスも一覧に載るため、載っていることだけで動かないとは断定しません
- これらのセルは「到達不能なら `done` を取り下げる」と書いているものがあり、素の exe で動かないと分かったときに
  identity が救済の経路になります。これは導入の利点の一つですが、今は仮説です

なお、identity とは別に、Windows 11 の Insider Beta（26220.9836）でデスクトップ アプリのカメラ・マイク・位置情報を
アプリ単位で管理する変更が試験されています。製品版に入るかは未定ですが、入れば素の exe の前提も変わります。

## 4. 選択肢の比較

| 観点 | 導入しない | sparse + 自己署名 | sparse + 有料署名 | フル MSIX |
| --- | --- | --- | --- | --- |
| 費用 | 無料 | 無料 | Azure Artifact Signing は月約 9.99 ドル（個人は米国・カナダのみ、組織は米国・カナダ・EU・英国）。対象外なら OV 証明書が年 150〜300 ドル。OSS 向けの無料枠（SignPath Foundation）は要件確認 | Store 経由は無料（開発者登録も無料で、Store が再署名）。Store 外は有料署名と同じ |
| 外れるセル | なし | 区分 A（見込み）。手元でのみ | 区分 A（見込み）。配布先でも | 区分 A（見込み）。Store なら区分 C の一部へ進める |
| 開発手順 | 変更なし | manifest の管理、MakeAppx・SignTool か winapp CLI、exe への埋め込み、Windows 上での登録。登録は実行ディレクトリに結び付くため、`target/debug`・`target/release`・worktree ごとに登録し直す | 自己署名に加えて本番証明書の鍵の管理 | パッケージ作成後に配置して起動する形へ変わる |
| CI | 変更なし | opt-in なら変更不要。CI で作るなら Windows runner が要り、`ci.md` の原則の例外を判断する | Windows runner と署名の Secrets | Windows runner と提出手順（#3） |
| 配布時の制約 | 変更なし | **公開配布不可**。公式は開発・試験用とし、一般利用者では SmartScreen が止めると記載 | 署名は通るが評判が溜まるまで SmartScreen の警告が出る。登録を行うインストーラーが要り、本リポジトリには無い | Store なら警告なし。Store の審査がある |
| 信頼登録の管理者権限 | 不要 | 公式の手順は `Cert:\CurrentUser\TrustedPeople` で足りると読める（未確認）。全ユーザーなら `LocalMachine` で昇格が要る。`windows-packaging-publish` skill は `LocalMachine` と書いており食い違う | 信頼済み CA なら不要 | Store なら不要 |
| 最低 Windows | 変更なし | Windows 10 2004（19041） | Windows 10 2004（19041） | 本書では未調査 |
| 既存セルへの影響 | なし | §3 | §3 | §3 に加え、既定の packaged desktop では HKCU と AppData の書き込みが仮想化されうる。スタートメニューの `.lnk` も対象に挙がる。exe の置き場も変わる |
| 撤退のしやすさ | — | 容易。`Remove-AppxPackage` と manifest の削除で戻る。opt-in なら既定のビルドに影響なし | 登録済みの利用者の解除が要る。契約の解約も要る | 難しい。配布形態ごと変わる |

補足:

- 署名なしで入れる `Add-AppxPackage -AllowUnsigned` もありますが、Windows 11 以降で、実行内容を含むなら管理者権限が要り、
  全ユーザーへ入ります。Publisher にも専用の OID が要り、利点が小さいため選択肢から外しました
- manifest の埋め込みは、`winapp embed-identity`（ビルド後に `mt.exe` で書き換える）なら依存クレートを増やさずに済みます。
  `build.rs` のリンカー引数で入れる方法は未検証です。手元の型検査で使う `x86_64-pc-windows-gnu` と MSVC で手段が分かれる見込みです
- 公式の `SignTool` の例はパスワードを `/p` で渡しますが、`security.md` は秘密情報を引数で受け取ることを禁じています。
  採用する場合は証明書ストア経由など別の渡し方にします。`.pfx` はリポジトリに置きません
- exe の `msix` 要素がパッケージと食い違う、または実行ディレクトリが登録先と違うと、identity が無いまま動くことがあります。
  公式のトラブルシューティングに記載があるため、identity の有無を実行時に判定し、無いときに成功を装わない必要があります
- 有料署名や Store へ移ると Publisher が変わる見込みで、PFN は Name と Publisher から作られるため別の identity になります。
  同意の記録、`deep-link` の JSON、`device-identifier` の値は引き継がれない前提で考えます

## 5. 判断の材料

- **得られるもの**: 区分 A の 6 セルの見込みと、identity 必須一覧に当たるセルの救済経路
- **失うもの**: 確認の組み合わせが増えます（素の exe / identity 付き）。opt-in でも、identity 付きで動くと書くセルは両方を確かめます
- **急ぐ理由の弱さ**: 区分 A は priority が後ろ寄りで、公開配布にはつながりません
- **急がない場合の費用**: 無料の確認経路を持たないと、`home-widget` などの区分 B がいつまでも確かめられません

## 6. 推し案（未決定）

**sparse package + 自己署名を、既定のビルドを変えない opt-in の確認経路として採る。着手は素の exe の Windows 実機確認の後にする。**

理由:

1. 無料で、`Remove-AppxPackage` で戻せます。試して駄目なら撤退の費用がほぼありません
2. 既定のビルドを素の exe のままにすれば、§3 の影響は identity 付きで動かしたときに限られます
3. 区分 B の「sparse で足りるか」は、この経路でしか安く確かめられません
4. フル MSIX は §4 のとおり既存セルへの影響が最大で、配布を決める #3 の着手時に判断すれば足ります
5. 実機確認を先にする理由は §3 のとおりです。`verified: true` が 0 件のまま identity を足すと、不具合の原因を切り分けられません

順序の案:

1. 素の exe で Windows の実機確認を進める（`device-check-pending` の確認用 issue）
2. identity 付きの最小の確認を 1 回行う（登録、`Package.Current`、§3 の表）。結果を `_/` に記録する
3. §3 の表で問題が無ければ、区分 A のうち priority の順に個別の issue で進める

この案の天井: 公開配布はできません。identity 付きでしか動かない機能は、利用者の手元では配布の経路が決まるまで使えません。

## 7. オーナーが判断する点

1. **sparse + 自己署名の opt-in 経路を採るか**。推し案は採る（素の exe の実機確認の後）。採らないなら区分 A・B は現状のまま据え置く
2. **identity 付きでだけ動くセルの表記**。推し案は `done` のまま note に「identity 付きのみ・自己署名の確認経路」と書く。
   別案は `possible` に据え置く。対応表の定義に関わるため、決めたら機能 PR より先に docs PR で定義を更新する
3. **配布の署名をいま決めるか**。推し案は決めない（契約もしない）。#3 の Windows に着手するときに Store と有料署名を比べる

## 8. 未確認事項

実機で確かめていない主な点です。

- sparse で区分 A の API が実際に動くか（`BackgroundUploader`、共有先、`windows.appUriHandler`、同期ルート、App Actions）
- `Cert:\CurrentUser\TrustedPeople` だけで `Add-AppxPackage` が通るか
- identity 付きのときの固有 AUMID の扱いと、既存 `.lnk` との関係
- full trust の sparse で `webcam` / `microphone` の宣言が要るか。同意プロンプトの出方
- 子プロセス（PowerShell 経由の機能）が identity を持つか
- 登録せずに `msix` 要素入りの exe を動かしたときの挙動（素の exe と同じか）
- identity の前後で `PasswordVault` の値を読めるか
- 自己署名で exe 本体の署名も要るか（公式の手順はパッケージの署名を求めるが、exe については読み取れなかった）

対応表の状態の揃え（`home-widget` に `deferred` が無い、`no` の中に #335 の基準で振り分け対象になるものがある）は本書の範囲外で、#335 で扱います。

## 9. 一次情報

- [Grant package identity by packaging with external location manually](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/grant-identity-to-nonpackaged-apps)
- [Grant package identity by packaging with external location（概要）](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/grant-identity-to-nonpackaged-apps-overview)
- [Sparse packaging（winapp CLI）](https://learn.microsoft.com/en-us/windows/apps/dev-tools/winapp-cli/guides/sparse)
- [Modernize packaged desktop apps（identity で使える機能）](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/modernize-packaged-apps)
- [WinRT APIs not supported in desktop apps（identity 必須一覧）](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/winrt-api-desktop-app-support)
- [Understanding how packaged desktop apps run on Windows（仮想化）](https://learn.microsoft.com/en-us/windows/msix/desktop/desktop-to-uwp-behind-the-scenes)
- [App capability declarations](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/app-capability-declarations)
- [Code signing options for Windows app developers](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options)
- [Create an unsigned MSIX package](https://learn.microsoft.com/en-us/windows/msix/package/unsigned-package)
- [Background transfers](https://learn.microsoft.com/en-us/windows/apps/develop/networking/background-transfers)
- [Enable apps for websites using app URI handlers](https://learn.microsoft.com/en-us/windows/apps/develop/launch/web-to-app-linking)
- [SystemIdentification.GetSystemIdForPublisher](https://learn.microsoft.com/en-us/uwp/api/windows.system.profile.systemidentification.getsystemidforpublisher)
- [Build a Cloud Sync Engine that Supports Placeholder Files](https://learn.microsoft.com/en-us/windows/win32/cfapi/build-a-cloud-file-sync-engine)
- [App Actions on Windows Overview](https://learn.microsoft.com/en-us/windows/ai/app-actions/)
- [Widget providers](https://learn.microsoft.com/en-us/windows/apps/develop/widgets/widget-providers)
- [Application User Model IDs](https://learn.microsoft.com/en-us/windows/win32/shell/appids)
- [Windows 11 Insider Beta Preview Build 26220.9836](https://learn.microsoft.com/en-us/windows-insider/release-notes/beta/preview-build-26220-9836)
- リポジトリ内: `.claude/skills/windows-packaging-publish/references/package-identity/`
