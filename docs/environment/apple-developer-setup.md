# Apple Developer Program 登録・証明書・プロビジョニングプロファイル取得手順

## 1. 目的・spec 対応

- **対応 Issue**: #12（chore(env): TASK-18.2 Apple Developer Program 登録・証明書・プロビジョニングプロファイル取得）
- **spec 対応**: REQ-18（開発環境整備、`docs/spec/04-requirements.md` L225-236）/ TASK-18.2（`docs/spec/05-tasks.md` L518-523）/ M1（`docs/spec/06-roadmap.md`）
- **本タスクの性質**: Apple Developer Program の登録・証明書発行・プロビジョニングプロファイル発行そのものは
  Apple アカウント保有者（人間）でなければ実行できない外部手続きである。本ドキュメントは
  CLAUDE.md の規約（「実機・証明書・外部手続きが必要なタスクは『人間担当』としてユーザーへ明示的に依頼する」）
  に従い、Claude 側で整備した手順書・チェックリスト・検証コマンドである。実際の登録・取得作業は
  人間が本ドキュメントに従って実施する。
- **後続タスクとの依存関係**: 本タスクの完了は以下のタスクの前提条件となる。
  - TASK-1.4（iOS 実機検証）・TASK-3.5（iOS プラットフォーム統合の実機検証）・TASK-9.2（iOS 実機性能計測）
    — M4（実機検証・性能ゲート）
  - TASK-12.1（notarization・ストア提出パイプライン）・TASK-12.4（秘密情報管理方針）
    — M5（ストア提出パイプライン完成）
- **並行タスク**: TASK-18.1（iOS/Android 実機調達、Issue #11）とは相互独立であり並行実施可能。
  ただしデバイス登録（本手順の 3-3）は実機到着後にのみ実施できるため、後追いで実施してよい。

## 2. 前提条件

- Apple ID（2 ファクタ認証必須）
- 登録種別の選択（個人 / 組織）
  - **個人（Individual）**: 開発者個人名でアプリが登録される
  - **組織（Organization）**: 法人名でアプリが登録される。D-U-N-S 番号（法人に登録された識別子、
    無料で取得可能）の提供が登録プロセス中に必須
  - どちらで登録するかは Bundle ID 命名（3-2 参照）と併せてユーザー判断事項とする
    （本ドキュメントでは選択肢の提示に留め、自動では確定しない）
- 年会費: 99 USD（またはご利用地域の現地通貨。非営利団体・教育機関・政府機関は免除対象になり得る。
  最新の金額・免除条件は一次情報を参照: <https://developer.apple.com/support/compare-memberships/>）
- macOS + Xcode 環境（証明書 CSR 作成・プロビジョニングプロファイルのインストール・Signing & Capabilities
  の確認に使用）

> 登録フロー・審査期間・必要書類は Apple 側の運用変更の影響を受けるため、着手時点で以下の一次情報を
> 必ず確認すること（本ドキュメントの記載内容より一次情報を優先する）。
>
> - Apple Developer Program 登録: <https://developer.apple.com/programs/enroll/>
> - 会員種別比較（年会費等）: <https://developer.apple.com/support/compare-memberships/>
> - 証明書: <https://developer.apple.com/support/certificates/>
> - App Store Connect API: `apple-distribution` リファレンススキル参照

## 3. 登録手順（人間実施）

### 3-1. Apple Developer Program 登録申請

1. <https://developer.apple.com/programs/enroll/> から登録申請を行う。
2. 個人 / 組織のいずれで登録するかを確定する（組織の場合は D-U-N-S 番号を用意する）。
3. 審査待ちが発生する（最短でも 1〜2 営業日、混雑時はさらに延伸し得る。工数外・暦日で律速するため
   M1 のクリティカルパスとして最優先で着手する。`docs/spec/06-roadmap.md` のリスク記述を参照）。

### 3-2. App ID（Bundle Identifier）の登録

- 命名案: `com.fandhe.multiplatform.<app-name>` のような階層的な命名を推奨するが、最終的な
  Bundle ID 命名はユーザー判断とする。
- Apple Developer ポータルの Certificates, Identifiers & Profiles → Identifiers から登録する。

### 3-3. デバイス登録（UDID）

- 手元の iOS 実機の UDID を Certificates, Identifiers & Profiles →
  Devices から登録する。
- 実機到着後にのみ実施可能なため、他の手順（3-1・3-2・3-4）と独立して後追い実施してよい。

### 3-4. 証明書取得（CSR 作成含む）

1. macOS の「キーチェーンアクセス」→ 証明書アシスタント → 「認証局に証明書を要求」から CSR
  （Certificate Signing Request）ファイル（`<placeholder>.certSigningRequest`）を作成する。
2. Apple Developer ポータル Certificates, Identifiers & Profiles → Certificates から証明書種別を選び、
   CSR をアップロードして発行する。
   - **Apple Development**（開発用証明書）: M4 の実機検証で使用
   - **Apple Distribution**（配布用証明書）: M5 のストア提出で使用
3. 発行された証明書（`.cer`）をダウンロードし、キーチェーンアクセスにインポートする
   （秘密鍵とペアで `.p12` としてエクスポートする場合はローカルのキーチェーンにのみ保管し、
   リポジトリには一切含めない。7 章参照）。
4. 証明書の有効期限を控え、失効前の再発行手順（Apple 公式ドキュメントの証明書失効・再発行フロー）
   へのポインタをチームで共有する。

### 3-5. プロビジョニングプロファイル発行

- Apple Developer ポータル Certificates, Identifiers & Profiles → Profiles から発行する。
  - **Development プロファイル**: App ID + デバイス（3-3 で登録した UDID）+ Apple Development 証明書
  - **App Store プロファイル**: App ID + Apple Distribution 証明書
- 発行後、プロファイル（`.mobileprovision`）をダウンロードし、Xcode にインストールする。

### 3-6.（任意・先行推奨）App Store Connect API キー発行

- TASK-12.1（notarytool によるノータライズ）で必要になるため、先行して取得しておくことを推奨する。
- App Store Connect → Users and Access → Integrations → App Store Connect API からキーを発行する。
- **ロールは最小権限とする**（Admin ではなく Developer ロールを推奨。OWASP A05 対策）。
- 発行される秘密鍵（`AuthKey_<KEY_ID>.p8`）はダウンロード後にローカルのキーチェーンまたは安全な
  シークレットストアに保管し、リポジトリ・CI 設定に一切含めない。
- 保管はローカルのキーチェーン、CI では GitHub Secrets 経由のみとする（7 章）。

## 4. 完了チェックリスト（Issue #12 受け入れ条件との対応）

`docs/spec/04-requirements.md` L231（REQ-18 受け入れ基準）と `docs/spec/06-roadmap.md`（M1 完了基準）の
該当項目に 1 対 1 で対応する。

| チェック項目 | 対応する spec 記述 |
|---|---|
| [ ] Apple Developer Program の登録が完了している（審査通過済み） | REQ-18 受け入れ基準「Apple Developer Program の登録・証明書・プロビジョニングプロファイルを取得する」/ M1 完了基準 |
| [ ] Apple Developer アカウントの年会費支払いが完了している | 同上 |
| [ ] App ID（Bundle Identifier）を登録済みである | 同上（証明書一式の前提） |
| [ ] Apple Development 証明書を取得済みである | 同上 |
| [ ] Apple Distribution 証明書を取得済みである | 同上 |
| [ ] Development プロビジョニングプロファイルを発行済みである | 同上 |
| [ ] App Store プロビジョニングプロファイルを発行済みである | 同上 |
| [ ]（任意）App Store Connect API キーを発行済みである | TASK-12.1 の前提整備 |
| [ ] 秘密鍵・プロファイル等をリポジトリにコミットしていないことを確認済みである | `.claude/rules/security.md`（秘密情報混入防止） |

すべてのチェックが完了したら、Issue #12 に完了報告コメントを投稿し、Issue をクローズする
（クローズは人間側の作業。5 章「検証コマンド」の出力を添える場合はシリアル番号・Team ID 等をマスクすること）。

## 5. 検証コマンド（macOS 上で人間が実行）

```bash
# 証明書がキーチェーンに存在することを確認する
security find-identity -v -p codesigning

# プロファイル内容（有効期限・App ID・デバイス UDID）を確認する
security cms -D -i <profile>.mobileprovision
```

- Xcode の Signing & Capabilities タブでエラーが出ていないことも確認する。
- 上記コマンドの出力を Issue コメント等に貼る場合は、証明書シリアル番号・Team ID をマスクすること
  （7 章参照）。

## 6. 完了報告時の注意

- Issue クローズの判断・実施は人間が行う。本ドキュメントの整備（Claude 側成果物）はマージをもって
  完了とし、実際の証明書取得完了は別途ユーザーが確認する。

## 7. 秘密情報の取り扱い（`.claude/rules/security.md` 準拠）

以下のファイルはリポジトリ・コミット・CI 設定に一切含めない（`.gitignore` で機械的にガード済み。
下記一覧および `.gitignore` を参照）。

- 秘密鍵を含む証明書エクスポート（`*.p12`）
- 証明書本体（`*.cer`）— 秘密鍵を伴わない公開証明書のみであれば機密性は低いが、誤って秘密鍵付き
  エクスポートを含めるリスクを避けるため一律除外する
- プロビジョニングプロファイル（`*.mobileprovision` / `*.provisionprofile`）
- CSR（`*.certSigningRequest`）
- App Store Connect API キー（`AuthKey_*.p8`）
- `.env` 系ファイル

保管先はローカルはキーチェーン（または OS のシークレットストア）、CI は GitHub Secrets 経由のみとする。
CI での署名シークレット利用の詳細方針は TASK-12.1 / TASK-12.4 側で策定する
（`.claude/rules/ci.md` の CI runner 前提・ログ出力禁止と整合させる）。

本ドキュメント中の識別子はすべてプレースホルダ（`<TEAM_ID>` / `<BUNDLE_ID>` / `<KEY_ID>` 等）であり、
実際の Team ID・Bundle ID・証明書シリアル番号を記載しない。
