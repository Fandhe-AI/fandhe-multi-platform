# `cargo fandhe run` / `build` の設計

利用者のアプリを、OS ごとの手順を知らずに 1 コマンドでビルド・起動するための CLI です（REQ-21 / TASK-21.5、#1201）。
OS ごとに PR を分けて足します（デスクトップ・Web #1422、Android #1423、iOS シミュレータ #1424、iOS 実機 #1425）。

## 形

```sh
cargo fandhe run|build [--os macos|windows|linux|web|android] [--release] [--manifest-path <アプリの Cargo.toml>]
```

- `--os` を省くとホストのデスクトップ OS です。同じオプションの重複と、`-` で始まる `--manifest-path` の値は引数エラー（2）です
- 終了コードは 0 正常 / 1 前提の不足・入力の不正・子プロセスの失敗 / 2 未知の引数です

## OS ごとの手順

| OS | build | run |
| --- | --- | --- |
| macos・windows・linux | `cargo build`。`--os` がホストと違えば止める | `cargo run` |
| android | gen（Android の分だけ）→ 殻を生成 → `cargo ndk -t <abi>... build --lib` → `gradle :app:assembleDebug`（`--release` は `:app:bundleRelease`、署名必須） | build のあと `adb install -r` → `adb shell am start -W -n <application-id>/com.fandhe.platform.MainActivity` → `adb shell pidof <application-id>`（`--release` は `:app:assembleRelease`） |
| web | `cargo build --lib --target wasm32-unknown-unknown` → 出力先を作り直す → `wasm-bindgen --target web` →（`--release` は `wasm-opt -Os`）→ `web/`・manifest・フォントを足す | build のあと、cargo-fandhe 自身が 127.0.0.1 の空きポートで配り、URL を出す（Ctrl-C で終了） |

- Web の出力先は `<target_dir>/fandhe/<package 名>/web/` です。`cargo build` が成功したあと、`wasm-bindgen` の前にこのディレクトリを消して作り直します（コンパイルに失敗しても前回の成果物は残ります）
  （`target_dir` の配下で、途中に symlink が無いことを確かめてから消します）。`wasm-bindgen` の出力は直下に置きます
- アプリの `web/` ディレクトリがあれば丸ごと写します
  （`web/` 自身が symlink なら無いものとして扱い、配下の symlink は写しません）。`web/index.html` が無ければ（symlink も無いものとして扱います）、全画面の canvas を持つ
  既定の `index.html` を出します。`[package.metadata.fandhe-app.web]` があれば、`web/` の写しのあとに
  `cargo fandhe gen --os web` と同じ manifest を置きます（テーブル以外の書き方は gen と同じエラーです）
- フォント（Noto Sans JP）は、依存に `fandhe-fonts` があれば
  （描画層は wasm32 以外でだけ依存するため、対象を絞らない依存解決から探します）`fonts/noto-sans-jp-regular.otf` へ写します。無ければ既定フォントで続行します
- 配信は GET だけで、配信ディレクトリ配下の通常ファイルだけを返します（`..`・絶対パス・ドライブ文字・symlink は 400）。
  `Host` が `127.0.0.1:<port>`・`localhost:<port>` 以外のリクエストも 400 にします（DNS rebinding 対策）。
  外部プロセス（python 等）を使わないため、cargo-fandhe が終わればポートも閉じます。127.0.0.1 の外へは開きません
- 手順は cargo-fandhe が持ちます。`scripts/` は本リポジトリの gallery 専用で、利用者には配られないため呼びません

### Android

- 殻（Gradle プロジェクト）は `<target_dir>/fandhe/<package 名>/android/` に作ります。
  `gen/` に置かないのは、gen が毎回 `gen/` を作り直して Gradle の `build/`・`.gradle/` を消すためです。
  殻のソース（`app/src`。`jniLibs` を含む）は毎回作り直し、ルートの Gradle ファイルは上書きします（前回の ABI の `.so` を APK に混ぜない）。Gradle のキャッシュは消しません
- 殻の書き込みと `gen` の出力先は、`target_dir` の配下で途中に symlink が無いことを確かめてから書きます（`Reset` と同じ検査）。
  Windows ホストでは `gradle.bat` を検査・起動します
- `am start` は Activity が見つからなくても終了コード 0 を返し、`Error:` を出力に出すだけです（エミュレータで確認）。そのため `-W` で起動完了まで待ち、
  続けて `adb shell pidof <application-id>` の段（`adb-check`）でプロセスがあることを確かめます。無ければ pidof が非 0 で、段名付きで失敗します
- `versionCode` は 1 固定です。Play へ 2 回目の提出（同じ applicationId で版を上げる）はできません。版を上げる設定は未対応で、必要になった時点で別 Issue にします
- 殻は最小です。`MainActivity`（`GameActivity` の継承と `System.loadLibrary(FandheLibrary.NAME)` だけ）・
  Manifest（`application` の属性だけ。MainActivity の宣言・LAUNCHER・権限は gen の Manifest を overlay で重ねる）・
  テーマ（`FandheAppTheme`）・アプリ名（`fandhe_app_name`。アプリの `res` と衝突しないよう接頭辞を付けています）。
  AGP・compileSdk・minSdk・targetSdk・build-tools は `platforms/android` と `scripts/android/versions.env` に揃えます
  （`tests/run.rs` が突き合わせます）。
  GameActivity の版は gen の `fandhe.gradle`
  （`fandhe-platform-android` の宣言）から入ります
- `applicationId` は `[package.metadata.fandhe-app.android] application-id` に書きます（必須。書式は `com.example.app`）。gen の出力には入りません
- `<application>` の属性は `application-attributes`（`android:` を除いた名前 → 値）で足します。殻の既定（`icon`・`label`・`theme`・`allowBackup`）はキー単位で置き換わります。
  `dataExtractionRules`・`fullBackupContent`・`debuggable` は書けません（前 2 つは `backup-exclusions`、`debuggable` はビルド種別が決めます）。
  `localeConfig`・`networkSecurityConfig` は、この欄に `@xml/...` の参照を書き、参照先の XML をアプリの `res`（`[package.metadata.fandhe-app.android] res`）に置けば足せます。
  権限・`res`・`java`・`gradle` などの欄は機能の宣言と同じで、gen の overlay に統合されます（`capability-declaration.md` §7）
- gen は Android の分だけ生成します（内部の絞り込み。`gen --os` の公開オプションではありません）。bin や iOS の bundle-id が無い Android 専用アプリでも止まりません。
  出力先はアプリの `gen/` ではなく `<target_dir>/fandhe/<package 名>/android/gen/` です。アプリの `gen/`（iOS・macOS の生成物や、印の無い利用者のディレクトリ）には触れません
- run は adb につながった端末 1 台の ABI だけ、build は arm64-v8a・armeabi-v7a・x86_64 を作ります。
  端末が 0 台・複数台なら止まります（複数のときは環境変数 `ANDROID_SERIAL`）。エミュレータは起動しません
- `--release` は `FANDHE_ANDROID_KEYSTORE`・`FANDHE_ANDROID_KEYSTORE_PASSWORD`・`FANDHE_ANDROID_KEY_ALIAS` が全部そろっていなければ止まります（無署名で成功させません）。
  値は環境変数のまま Gradle へ継承します。keystore のパスだけ、Gradle の作業ディレクトリに依らないよう絶対パスにして上書きします。
  `build --release` は `.aab`、`run --release` は `.apk` です
- Gradle wrapper は同梱せず、PATH の `gradle` を使います（`platforms/android` と同じ。wrapper jar のバイナリをコミット・生成物に入れない）

## 前提の検査

- 実行前に不足を全件集め、1 件 1 行で「何が無いか。直し方」を出します。環境を黙って変えません（`rustup target add` などは案内だけ）
- 見るもの: Rust の target（rustup があるとき）、bin（デスクトップ）・cdylib（Web・Android）の有無、`wasm-bindgen` CLI の有無と
  Cargo.lock の `wasm-bindgen` との版の一致、`--release` の `wasm-opt`。Android は application-id、`ANDROID_HOME`（または `ANDROID_SDK_ROOT`）、NDK、
  `cargo-ndk`・`gradle`・`java`、run は `adb` と端末 1 台、`--release` は署名の環境変数と keystore ファイルの実在
- エラーに環境変数の値を出しません（`security.md`）
- 子プロセスの失敗は出力をそのまま流し、最後にどの段（cargo・wasm-bindgen・wasm-opt・cargo-ndk・gradle・adb-install・adb-start・adb-check）で止まったかを 1 行出します

## 作り

`crates/cargo-fandhe/src/run/` は 3 層です。

- `prepare`: アプリ情報・オプション・環境のスナップショットから、不足の一覧か手順（書くファイル・実行するコマンドの列）を返す純粋関数。
  `crates/cargo-fandhe/tests/run.rs` が OS ごとの手順と検査の分岐を固定します
- `load_app`・`detect_env`: `cargo metadata` と実環境から入力を作ります
- `execute`: 手順を順に実行するだけです

## eject との関係（#1202）

`run`/`build --os android` の gen 出力も `eject/` を通ります（`load_app` が差し替えて「eject 済み: `<rel>`」を stderr に出す）。
`eject/ios/`・`eject/macos/` 配下の中身は `--os android` では検査せず、それらを理由に止めません。
`eject/` 直下の余分なファイルや、`eject/android/` の綴り違いでは止まります。
`eject/` を辿る深さの上限（8 段）は OS で絞り込む前に効くため、`--os android` でも深すぎる `eject/ios/...` では止まります。手順は
`capability-declaration.md` §7 を参照。

## 範囲外

- Windows の signtool による署名とパッケージ ID（#1208）
- Android の殻の重ね置き。追記は Apple の `info-plist`・`entitlements` と Android の宣言・`application-attributes`、設定ファイルの `--eject` が実装済み
- 既存の `scripts/` を CLI 呼び出しへ寄せること
