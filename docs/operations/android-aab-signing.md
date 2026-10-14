# 運用手順: Android 開発署名鍵による `.aab` 署名

- 対応: REQ-11（ストア提出パイプライン・開発署名まで）/ TASK-11.2-S3（M3）、Issue #227。
- 親: TASK-11.2 系列（親 issue #224）。sub-issue 構成 S1（#225: `.aab` 生成パイプライン
  CLI 構成設計、設計書 `docs/architecture/android-aab-pipeline.md`）/ S2（#226: cargo-ndk →
  Gradle 統合・未署名 `.aab` 生成、`scripts/android/build-aab.sh`）/ S3（本書、開発署名）/
  S4（#228: CI 組み込み・パイプライン動作確認）。

## 1. 位置付けと現状のスコープ

`scripts/android/build-aab.sh` は S2 で確立した「cargo-ndk クロスビルド →
`gradle bundleRelease`」の単一スクリプトに、本 S3 で開発用 keystore による
Gradle `signingConfigs` 署名を追加した（S1 設計 §5 で第一候補と確定済みの方式。
`jarsigner` 後段署名方式は不採用）。環境変数 `FANDHE_ANDROID_KEYSTORE` が
未設定の場合は S2 までと同じ未署名 `.aab` を生成する（後方互換）。

スコープ境界（重要）:

- **開発用署名のみ**を扱う。実配布署名（Play Console アップロード鍵・REQ-12）は
  将来スコープであり、本書・本実装の対象外。開発署名鍵と実配布鍵は明確に別物として
  管理する（8 節参照）。
- **CI（`.github/workflows/ci.yml`）への組み込みは行わない**（TASK-11.2-S4 / #228 へ
  申し送る。GitHub Secrets 経由での鍵配布も S4 スコープ）。
- **エミュレータへのインストール・起動確認はしない**（TASK-11.3 / #246 のスコープ）。
- keystore 生成ヘルパースクリプトの新設は行わない（S1 設計の成果物一覧「手順の
  ドキュメント化」を超えるため。本書の手順を直接実行する）。

## 2. 開発用 keystore の生成手順

`keytool` は `scripts/android/android-env.sh` が `export` する `JAVA_HOME`
（Temurin JDK 21.0.12+8、`scripts/android/versions.env` で固定）配下のものを使う。

```bash
# JAVA_HOME 等の環境変数を読み込む
. scripts/android/android-env.sh

# PKCS12 形式の keystore を生成する（storetype はビルド側の既定と合わせて PKCS12 を推奨）。
# -validity は日数（10000 日 ≒ 27 年、開発用途として十分長い有効期限）。
"${JAVA_HOME}/bin/keytool" -genkeypair \
  -keystore /path/to/fandhe-dev.jks \
  -alias fandhe-dev \
  -keyalg RSA -keysize 2048 \
  -validity 10000 \
  -storetype PKCS12
```

実行すると氏名・組織等の DN 情報とパスフレーズの入力を対話プロンプトで求められる。
**パスフレーズはこの対話プロンプトでのみ入力し、シェルの引数・環境変数の一括代入
（`export FOO=bar` をコマンド履歴に残す形）・スクリプトへの埋め込みでは行わない**
（シェル履歴・プロセス一覧への露出防止、security.md 準拠）。

生成した `.jks` ファイル自体は `.gitignore`（`*.jks` / `*.keystore`）で除外済みのため、
誤ってリポジトリへコミットされることはない。念のためコミット前に `git status` で
意図しないファイルが含まれていないことを確認すること。

## 3. 環境変数インターフェース

`scripts/android/build-aab.sh`（S1 の CLI からもこの環境変数経由で呼び出される契約。
`scripts/ios/build-simulator-app.sh` の `FANDHE_IOS_*` と同じ設計方針）:

| 環境変数 | 既定値 | 説明 |
|----------|--------|------|
| `FANDHE_ANDROID_KEYSTORE` | 空（署名しない） | 開発署名鍵（`.jks` / PKCS12）のパス。非空の場合のみ署名する |
| `FANDHE_ANDROID_KEYSTORE_PASSWORD` | 空 | keystore パスフレーズ。`FANDHE_ANDROID_KEYSTORE` 非空時は必須（部分設定は fail-closed でエラーになる） |
| `FANDHE_ANDROID_KEY_ALIAS` | `fandhe-dev` | 鍵エイリアス |
| `FANDHE_ANDROID_KEY_PASSWORD` | `FANDHE_ANDROID_KEYSTORE_PASSWORD` と同値 | 鍵パスワード（PKCS12 では store と同一が前提のため既定は流用） |

パスワード類は Gradle へ `-P` プロパティで渡さない（プロセス一覧・ログへの露出を
避けるため）。`platforms/android/app/build.gradle.kts` 側で `System.getenv()` に
より直接読み、`signingConfigs` へ設定する。

## 4. 使い方

### 4.1 未署名モード（既定・S2 までと同じ挙動）

```bash
scripts/android/build-aab.sh
```

`FANDHE_ANDROID_KEYSTORE` 未設定時は署名を一切行わず、`.aab` 内に署名ブロック
（`META-INF/*.RSA` 等）が存在しないことをスクリプト自身が検証する。

### 4.2 開発署名モード

```bash
FANDHE_ANDROID_KEYSTORE=/path/to/fandhe-dev.jks \
FANDHE_ANDROID_KEYSTORE_PASSWORD='<keystore のパスフレーズ>' \
  scripts/android/build-aab.sh
```

成功すると `platforms/android/app/build/outputs/bundle/release/app-release.aab` が
開発署名鍵で署名された状態で生成される。スクリプトは以下を機械検証する。

- `.aab` 内 `META-INF/` 配下に署名ブロック（`.RSA` 等）が存在すること。
- `"${JAVA_HOME}/bin/jarsigner" -verify` が成功すること（"jar verified"）。

### 4.3 異常系（fail-closed）

| 状況 | 挙動 |
|------|------|
| `FANDHE_ANDROID_KEYSTORE` のみ設定・パスワード未設定 | cargo ビルド前に即エラー（exit 1） |
| `FANDHE_ANDROID_KEYSTORE_PASSWORD` のみ設定・keystore 未設定 | cargo ビルド前に即エラー（exit 1） |
| 指定した keystore パスにファイルが存在しない | cargo ビルド前に即エラー（exit 1） |
| パスワードが誤っている | Gradle の署名処理が失敗し非 0 終了（エラーメッセージにパスワードは含まれない） |

「keystore だけ設定してパスワード無しで未署名のまま成功させる」ことは行わない
（A05「セキュリティ設定ミス」対策。署名したつもりで未署名という事故を防ぐ）。

## 5. 秘密情報の取り扱い

- keystore（`.jks` / `.keystore`）とパスフレーズはリポジトリへコミットしない
  （`.gitignore` 追記済み・`create-commit` スキルのシークレット検査対象）。
- 受け渡しは環境変数のみ。Gradle `-P` プロパティ・コマンドライン引数・
  `build.gradle.kts` への直書きは行わない。
- CI（GitHub Actions）での利用は **GitHub Secrets 経由のみ**とする。
  実際の CI ワークフローへの組み込みは TASK-11.2-S4（#228）のスコープであり、
  本書の環境変数インターフェースをそのまま Secrets → 環境変数の橋渡しに使う想定。
- ログ・エラーメッセージへパスワードを出力しない（`build-aab.sh` は `set -x` を
  使用せず、パスワード変数を `log()` / `fail()` の引数へ渡さない）。

## 6. 開発署名鍵と実配布鍵の分界

| | 開発署名鍵（本書のスコープ） | 実配布鍵（将来スコープ・REQ-12） |
|---|---|---|
| 目的 | ローカル・CI での動作確認用 `.aab` 生成 | Play Console へのアップロード・ストア配布 |
| 管理場所 | 開発者ローカル / CI シークレットストア（GitHub Secrets） | Play App Signing（Google 管理）または組織の鍵管理基盤 |
| ライフサイクル | チーム内で再生成・ローテーション可 | 原則不変（紛失・漏洩時の影響が大きい） |

本書・本実装は開発署名鍵のみを対象とし、実配布鍵の生成・管理手順には踏み込まない。

実配布鍵（Play アップロード鍵）の管理は本書の対象外（Play App Signing 等、6 節の表のとおり）。
