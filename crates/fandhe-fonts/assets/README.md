# 同梱フォント資産（TASK-10.1 / REQ-10 / M3）

## NotoSansJP-Regular.otf

| 項目 | 値 |
|------|----|
| 出典 | [notofonts/noto-cjk](https://github.com/notofonts/noto-cjk) 公式リリース |
| リリースタグ | `Sans2.004`（Noto Sans CJK Version 2.004） |
| 取得元アセット | `16_NotoSansJP.zip`（<https://github.com/notofonts/noto-cjk/releases/download/Sans2.004/16_NotoSansJP.zip> 内の `NotoSansJP-Regular.otf`） |
| ウェイト | Regular 1 種のみ（本タスクの採用範囲。他ウェイトは未採用） |
| 形式 | OpenType（CFF アウトライン、拡張子 `.otf`）。静的インスタンス（バリアブルフォントではない） |
| ファイルサイズ | 4,533,028 バイト（4.5 MiB） |
| SHA-256 | `dff723ba59d57d136764a04b9b2d03205544f7cd785a711442d6d2d085ac5073` |
| ライセンス | SIL Open Font License, Version 1.1（`OFL.txt` に全文を同梱） |
| 著作権表記 | © 2014-2021 Adobe (http://www.adobe.com/). Noto is a trademark of Google Inc.（フォントの name テーブル ID 0 / 7。配布元 LICENSE に著作権行はなく、`OFL.txt` は配布元 `Sans2.004` の LICENSE と同一） |

### フォーマット選定の根拠（TTF ではなく OTF を採用した理由）

計画時点では静的 TTF を想定していたが、`notofonts/noto-cjk` の公式リリースで配布される
単一ウェイトの静的フォントは OTF（CFF アウトライン）形式のみであり、TTF（glyf アウトライン）の
単一ウェイト静的版は upstream に存在しない（Google Fonts 側も現在はバリアブルフォント
`NotoSansJP[wght].ttf` のみを配布しており、静的インスタンスではない）。

本クレートが埋め込むフォントの実際の消費者は egui/epaint であり、そのフォントパーサ・
ラスタライザは `ab_glyph`（内部で `owned_ttf_parser` を使用）である。`owned_ttf_parser` は
CFF アウトラインをコア機能として扱うため、OTF のままでも問題なく解釈・描画できることを
`tests/glyph_coverage.rs`（`fandhe-fonts`）で実際に `ab_glyph::FontRef::try_from_slice` に
通し、代表的な CJK 文字のグリフをラスタライズして確認済み。ライセンス・出典・再配布条件は
TTF/OTF いずれでも同一（SIL OFL 1.1）であり、REQ-10 の受け入れ基準（CJK 対応フォントを
既定の同梱フォントセットに含め、日本語テキストが正しくレンダリングされることを確認する）を
満たす上で形式の違いは影響しない。

### WASM 配信サイズへの影響（参考情報）

本タスクのスコープはフォント同梱と日本語レンダリング確認までであり、WASM 配信サイズへの
影響計測・3 MB 基準（REQ-4）とのトレードオフ判断は TASK-10.2（別 issue）のスコープとする。
参考として、Web 経路（`scripts/web/build-wasm-proxy.sh`）が `dist/fonts/` へコピーする
フォントファイルのサイズは上記 4,533,028 バイトそのものであり、TASK-10.2 のサイズ計測時に
本ファイルを起点にすること。

### 整合性検証手順（再取得時）

```bash
curl -sL -o NotoSansJP-Regular.otf \
  https://github.com/notofonts/noto-cjk/releases/download/Sans2.004/16_NotoSansJP.zip
# unzip して NotoSansJP-Regular.otf を取り出した後
sha256sum NotoSansJP-Regular.otf
# dff723ba59d57d136764a04b9b2d03205544f7cd785a711442d6d2d085ac5073 と一致することを確認する
```
