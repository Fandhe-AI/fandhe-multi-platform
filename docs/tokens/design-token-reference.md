# デザイントークンリファレンス（自動生成）

**このファイルは自動生成されたものです。手動で編集しないでください。**

再生成コマンド:

```sh
cargo run -p fandhe-core --example generate_token_docs
```

`crates/fandhe-core/src/token/docgen.rs`（TASK-8.4-S20、#176）がコード上の
トークン定義から本ファイルを生成します。コードとドキュメントの乖離は
`cargo test -p fandhe-core --test token_docgen` の鮮度検証テストが検出します。

## スキーマ概要

### カテゴリ一覧

- Color
- Dimension
- Text
- Typography
- Spacing
- Size
- Radius
- Elevation
- Border
- Shadow
- Duration
- Animation

### 命名規則

トークンパスは `category.group.name` 形式のドット区切り階層パスです
（例: `color.primary`、`spacing.md`、`duration.normal`）。

## パレット

| トークンパス | カテゴリ | 定義 | 解決後の値 |
|---|---|---|---|
| `color.palette.black` | Color | `#000000ff`（rgba 0, 0, 0, 255） | (直接値) |
| `color.palette.blue-100` | Color | `#dbeafeff`（rgba 219, 234, 254, 255） | (直接値) |
| `color.palette.blue-200` | Color | `#bfdbfeff`（rgba 191, 219, 254, 255） | (直接値) |
| `color.palette.blue-300` | Color | `#93c5fdff`（rgba 147, 197, 253, 255） | (直接値) |
| `color.palette.blue-400` | Color | `#60a5faff`（rgba 96, 165, 250, 255） | (直接値) |
| `color.palette.blue-50` | Color | `#eff6ffff`（rgba 239, 246, 255, 255） | (直接値) |
| `color.palette.blue-500` | Color | `#3b82f6ff`（rgba 59, 130, 246, 255） | (直接値) |
| `color.palette.blue-600` | Color | `#2563ebff`（rgba 37, 99, 235, 255） | (直接値) |
| `color.palette.blue-700` | Color | `#1d4ed8ff`（rgba 29, 78, 216, 255） | (直接値) |
| `color.palette.blue-800` | Color | `#1e40afff`（rgba 30, 64, 175, 255） | (直接値) |
| `color.palette.blue-900` | Color | `#1e3a8aff`（rgba 30, 58, 138, 255） | (直接値) |
| `color.palette.gray-100` | Color | `#f5f5f5ff`（rgba 245, 245, 245, 255） | (直接値) |
| `color.palette.gray-200` | Color | `#e5e5e5ff`（rgba 229, 229, 229, 255） | (直接値) |
| `color.palette.gray-300` | Color | `#d4d4d4ff`（rgba 212, 212, 212, 255） | (直接値) |
| `color.palette.gray-400` | Color | `#a3a3a3ff`（rgba 163, 163, 163, 255） | (直接値) |
| `color.palette.gray-50` | Color | `#fafafaff`（rgba 250, 250, 250, 255） | (直接値) |
| `color.palette.gray-500` | Color | `#737373ff`（rgba 115, 115, 115, 255） | (直接値) |
| `color.palette.gray-600` | Color | `#525252ff`（rgba 82, 82, 82, 255） | (直接値) |
| `color.palette.gray-700` | Color | `#404040ff`（rgba 64, 64, 64, 255） | (直接値) |
| `color.palette.gray-800` | Color | `#262626ff`（rgba 38, 38, 38, 255） | (直接値) |
| `color.palette.gray-900` | Color | `#171717ff`（rgba 23, 23, 23, 255） | (直接値) |
| `color.palette.green-100` | Color | `#dcfce7ff`（rgba 220, 252, 231, 255） | (直接値) |
| `color.palette.green-200` | Color | `#bbf7d0ff`（rgba 187, 247, 208, 255） | (直接値) |
| `color.palette.green-300` | Color | `#86efacff`（rgba 134, 239, 172, 255） | (直接値) |
| `color.palette.green-400` | Color | `#4ade80ff`（rgba 74, 222, 128, 255） | (直接値) |
| `color.palette.green-50` | Color | `#f0fdf4ff`（rgba 240, 253, 244, 255） | (直接値) |
| `color.palette.green-500` | Color | `#22c55eff`（rgba 34, 197, 94, 255） | (直接値) |
| `color.palette.green-600` | Color | `#16a34aff`（rgba 22, 163, 74, 255） | (直接値) |
| `color.palette.green-700` | Color | `#15803dff`（rgba 21, 128, 61, 255） | (直接値) |
| `color.palette.green-800` | Color | `#166534ff`（rgba 22, 101, 52, 255） | (直接値) |
| `color.palette.green-900` | Color | `#14532dff`（rgba 20, 83, 45, 255） | (直接値) |
| `color.palette.red-100` | Color | `#fee2e2ff`（rgba 254, 226, 226, 255） | (直接値) |
| `color.palette.red-200` | Color | `#fecacaff`（rgba 254, 202, 202, 255） | (直接値) |
| `color.palette.red-300` | Color | `#fca5a5ff`（rgba 252, 165, 165, 255） | (直接値) |
| `color.palette.red-400` | Color | `#f87171ff`（rgba 248, 113, 113, 255） | (直接値) |
| `color.palette.red-50` | Color | `#fef2f2ff`（rgba 254, 242, 242, 255） | (直接値) |
| `color.palette.red-500` | Color | `#ef4444ff`（rgba 239, 68, 68, 255） | (直接値) |
| `color.palette.red-600` | Color | `#dc2626ff`（rgba 220, 38, 38, 255） | (直接値) |
| `color.palette.red-700` | Color | `#b91c1cff`（rgba 185, 28, 28, 255） | (直接値) |
| `color.palette.red-800` | Color | `#991b1bff`（rgba 153, 27, 27, 255） | (直接値) |
| `color.palette.red-900` | Color | `#7f1d1dff`（rgba 127, 29, 29, 255） | (直接値) |
| `color.palette.transparent` | Color | `#00000000`（rgba 0, 0, 0, 0） | (直接値) |
| `color.palette.white` | Color | `#ffffffff`（rgba 255, 255, 255, 255） | (直接値) |
| `color.palette.yellow-100` | Color | `#fef9c3ff`（rgba 254, 249, 195, 255） | (直接値) |
| `color.palette.yellow-200` | Color | `#fef08aff`（rgba 254, 240, 138, 255） | (直接値) |
| `color.palette.yellow-300` | Color | `#fde047ff`（rgba 253, 224, 71, 255） | (直接値) |
| `color.palette.yellow-400` | Color | `#facc15ff`（rgba 250, 204, 21, 255） | (直接値) |
| `color.palette.yellow-50` | Color | `#fefce8ff`（rgba 254, 252, 232, 255） | (直接値) |
| `color.palette.yellow-500` | Color | `#eab308ff`（rgba 234, 179, 8, 255） | (直接値) |
| `color.palette.yellow-600` | Color | `#ca8a04ff`（rgba 202, 138, 4, 255） | (直接値) |
| `color.palette.yellow-700` | Color | `#a16207ff`（rgba 161, 98, 7, 255） | (直接値) |
| `color.palette.yellow-800` | Color | `#854d0eff`（rgba 133, 77, 14, 255） | (直接値) |
| `color.palette.yellow-900` | Color | `#713f12ff`（rgba 113, 63, 18, 255） | (直接値) |

## タイポグラフィ

| トークンパス | カテゴリ | 定義 | 解決後の値 |
|---|---|---|---|
| `typography.body` | Typography | family=sans-serif, size=16px, line_height=22.4px, weight=400, style=normal | (直接値) |
| `typography.caption` | Typography | family=sans-serif, size=12px, line_height=16.8px, weight=400, style=normal | (直接値) |
| `typography.display` | Typography | family=sans-serif, size=36px, line_height=50.4px, weight=400, style=normal | (直接値) |
| `typography.headline` | Typography | family=sans-serif, size=28px, line_height=39.2px, weight=400, style=normal | (直接値) |
| `typography.label` | Typography | family=sans-serif, size=14px, line_height=19.6px, weight=400, style=normal | (直接値) |
| `typography.title` | Typography | family=sans-serif, size=22px, line_height=30.8px, weight=400, style=normal | (直接値) |

## スペーシング

| トークンパス | カテゴリ | 定義 | 解決後の値 |
|---|---|---|---|
| `spacing.lg` | Spacing | 24px | (直接値) |
| `spacing.md` | Spacing | 16px | (直接値) |
| `spacing.none` | Spacing | 0px | (直接値) |
| `spacing.sm` | Spacing | 8px | (直接値) |
| `spacing.xl` | Spacing | 32px | (直接値) |
| `spacing.xs` | Spacing | 4px | (直接値) |
| `spacing.xxl` | Spacing | 48px | (直接値) |
| `spacing.xxxl` | Spacing | 64px | (直接値) |

## サイズ

| トークンパス | カテゴリ | 定義 | 解決後の値 |
|---|---|---|---|
| `size.lg` | Size | 48px | (直接値) |
| `size.md` | Size | 32px | (直接値) |
| `size.sm` | Size | 24px | (直接値) |
| `size.xl` | Size | 64px | (直接値) |
| `size.xs` | Size | 16px | (直接値) |

## 角丸（radius）

| トークンパス | カテゴリ | 定義 | 解決後の値 |
|---|---|---|---|
| `radius.full` | Radius | 9999px | (直接値) |
| `radius.lg` | Radius | 16px | (直接値) |
| `radius.md` | Radius | 8px | (直接値) |
| `radius.none` | Radius | 0px | (直接値) |
| `radius.sm` | Radius | 4px | (直接値) |

## 高さ（elevation）

| トークンパス | カテゴリ | 定義 | 解決後の値 |
|---|---|---|---|
| `elevation.flat` | Elevation | level 0 | (直接値) |
| `elevation.floating` | Elevation | level 2 | (直接値) |
| `elevation.modal` | Elevation | level 4 | (直接値) |
| `elevation.overlay` | Elevation | level 3 | (直接値) |
| `elevation.raised` | Elevation | level 1 | (直接値) |
| `elevation.top` | Elevation | level 5 | (直接値) |

## ボーダー

| トークンパス | カテゴリ | 定義 | 解決後の値 |
|---|---|---|---|
| `border.dashed` | Border | width=1px, style=dashed, color=`#d4d4d4ff`（rgba 212, 212, 212, 255）（参照元: `color.palette.gray-300`） | (直接値) |
| `border.default` | Border | width=1px, style=solid, color=`#d4d4d4ff`（rgba 212, 212, 212, 255）（参照元: `color.palette.gray-300`） | (直接値) |
| `border.strong` | Border | width=2px, style=solid, color=`#737373ff`（rgba 115, 115, 115, 255）（参照元: `color.palette.gray-500`） | (直接値) |

## シャドウ

| トークンパス | カテゴリ | 定義 | 解決後の値 |
|---|---|---|---|
| `shadow.lg` | Shadow | offset=(0, 8)px, blur=16px, color=`#000000ff`（rgba 0, 0, 0, 255）（参照元: `color.palette.black`） | (直接値) |
| `shadow.md` | Shadow | offset=(0, 4)px, blur=8px, color=`#000000ff`（rgba 0, 0, 0, 255）（参照元: `color.palette.black`） | (直接値) |
| `shadow.sm` | Shadow | offset=(0, 1)px, blur=2px, color=`#000000ff`（rgba 0, 0, 0, 255）（参照元: `color.palette.black`） | (直接値) |

## duration

| トークンパス | カテゴリ | 定義 | 解決後の値 |
|---|---|---|---|
| `duration.fast` | Duration | 100ms | (直接値) |
| `duration.instant` | Duration | 0ms | (直接値) |
| `duration.normal` | Duration | 250ms | (直接値) |
| `duration.slow` | Duration | 400ms | (直接値) |
| `duration.slower` | Duration | 700ms | (直接値) |

## アニメーション

| トークンパス | カテゴリ | 定義 | 解決後の値 |
|---|---|---|---|
| `animation.emphasized` | Animation | duration=400ms, easing=ease-in-out | (直接値) |
| `animation.enter` | Animation | duration=250ms, easing=ease-out | (直接値) |
| `animation.exit` | Animation | duration=100ms, easing=ease-in | (直接値) |
| `animation.standard` | Animation | duration=250ms, easing=ease-in-out | (直接値) |

## iOS 基本テーマ（ライト、セマンティックカラーを含む）

| トークンパス | カテゴリ | 定義 | 解決後の値 |
|---|---|---|---|
| `color.background` | Color | → `color.palette.ios-background`（期待カテゴリ: Color） | `#ffffffff`（rgba 255, 255, 255, 255） |
| `color.error` | Color | → `color.palette.ios-red`（期待カテゴリ: Color） | `#ff3b30ff`（rgba 255, 59, 48, 255） |
| `color.on-background` | Color | → `color.palette.ios-label`（期待カテゴリ: Color） | `#000000ff`（rgba 0, 0, 0, 255） |
| `color.on-primary` | Color | → `color.palette.ios-on-accent`（期待カテゴリ: Color） | `#ffffffff`（rgba 255, 255, 255, 255） |
| `color.on-surface` | Color | → `color.palette.ios-label`（期待カテゴリ: Color） | `#000000ff`（rgba 0, 0, 0, 255） |
| `color.palette.black` | Color | `#000000ff`（rgba 0, 0, 0, 255） | (直接値) |
| `color.palette.blue-100` | Color | `#dbeafeff`（rgba 219, 234, 254, 255） | (直接値) |
| `color.palette.blue-200` | Color | `#bfdbfeff`（rgba 191, 219, 254, 255） | (直接値) |
| `color.palette.blue-300` | Color | `#93c5fdff`（rgba 147, 197, 253, 255） | (直接値) |
| `color.palette.blue-400` | Color | `#60a5faff`（rgba 96, 165, 250, 255） | (直接値) |
| `color.palette.blue-50` | Color | `#eff6ffff`（rgba 239, 246, 255, 255） | (直接値) |
| `color.palette.blue-500` | Color | `#3b82f6ff`（rgba 59, 130, 246, 255） | (直接値) |
| `color.palette.blue-600` | Color | `#2563ebff`（rgba 37, 99, 235, 255） | (直接値) |
| `color.palette.blue-700` | Color | `#1d4ed8ff`（rgba 29, 78, 216, 255） | (直接値) |
| `color.palette.blue-800` | Color | `#1e40afff`（rgba 30, 64, 175, 255） | (直接値) |
| `color.palette.blue-900` | Color | `#1e3a8aff`（rgba 30, 58, 138, 255） | (直接値) |
| `color.palette.gray-100` | Color | `#f5f5f5ff`（rgba 245, 245, 245, 255） | (直接値) |
| `color.palette.gray-200` | Color | `#e5e5e5ff`（rgba 229, 229, 229, 255） | (直接値) |
| `color.palette.gray-300` | Color | `#d4d4d4ff`（rgba 212, 212, 212, 255） | (直接値) |
| `color.palette.gray-400` | Color | `#a3a3a3ff`（rgba 163, 163, 163, 255） | (直接値) |
| `color.palette.gray-50` | Color | `#fafafaff`（rgba 250, 250, 250, 255） | (直接値) |
| `color.palette.gray-500` | Color | `#737373ff`（rgba 115, 115, 115, 255） | (直接値) |
| `color.palette.gray-600` | Color | `#525252ff`（rgba 82, 82, 82, 255） | (直接値) |
| `color.palette.gray-700` | Color | `#404040ff`（rgba 64, 64, 64, 255） | (直接値) |
| `color.palette.gray-800` | Color | `#262626ff`（rgba 38, 38, 38, 255） | (直接値) |
| `color.palette.gray-900` | Color | `#171717ff`（rgba 23, 23, 23, 255） | (直接値) |
| `color.palette.green-100` | Color | `#dcfce7ff`（rgba 220, 252, 231, 255） | (直接値) |
| `color.palette.green-200` | Color | `#bbf7d0ff`（rgba 187, 247, 208, 255） | (直接値) |
| `color.palette.green-300` | Color | `#86efacff`（rgba 134, 239, 172, 255） | (直接値) |
| `color.palette.green-400` | Color | `#4ade80ff`（rgba 74, 222, 128, 255） | (直接値) |
| `color.palette.green-50` | Color | `#f0fdf4ff`（rgba 240, 253, 244, 255） | (直接値) |
| `color.palette.green-500` | Color | `#22c55eff`（rgba 34, 197, 94, 255） | (直接値) |
| `color.palette.green-600` | Color | `#16a34aff`（rgba 22, 163, 74, 255） | (直接値) |
| `color.palette.green-700` | Color | `#15803dff`（rgba 21, 128, 61, 255） | (直接値) |
| `color.palette.green-800` | Color | `#166534ff`（rgba 22, 101, 52, 255） | (直接値) |
| `color.palette.green-900` | Color | `#14532dff`（rgba 20, 83, 45, 255） | (直接値) |
| `color.palette.ios-background` | Color | `#ffffffff`（rgba 255, 255, 255, 255） | (直接値) |
| `color.palette.ios-blue` | Color | `#007affff`（rgba 0, 122, 255, 255） | (直接値) |
| `color.palette.ios-gray` | Color | `#8e8e93ff`（rgba 142, 142, 147, 255） | (直接値) |
| `color.palette.ios-green` | Color | `#34c759ff`（rgba 52, 199, 89, 255） | (直接値) |
| `color.palette.ios-label` | Color | `#000000ff`（rgba 0, 0, 0, 255） | (直接値) |
| `color.palette.ios-on-accent` | Color | `#ffffffff`（rgba 255, 255, 255, 255） | (直接値) |
| `color.palette.ios-orange` | Color | `#ff9500ff`（rgba 255, 149, 0, 255） | (直接値) |
| `color.palette.ios-red` | Color | `#ff3b30ff`（rgba 255, 59, 48, 255） | (直接値) |
| `color.palette.ios-surface` | Color | `#f2f2f7ff`（rgba 242, 242, 247, 255） | (直接値) |
| `color.palette.red-100` | Color | `#fee2e2ff`（rgba 254, 226, 226, 255） | (直接値) |
| `color.palette.red-200` | Color | `#fecacaff`（rgba 254, 202, 202, 255） | (直接値) |
| `color.palette.red-300` | Color | `#fca5a5ff`（rgba 252, 165, 165, 255） | (直接値) |
| `color.palette.red-400` | Color | `#f87171ff`（rgba 248, 113, 113, 255） | (直接値) |
| `color.palette.red-50` | Color | `#fef2f2ff`（rgba 254, 242, 242, 255） | (直接値) |
| `color.palette.red-500` | Color | `#ef4444ff`（rgba 239, 68, 68, 255） | (直接値) |
| `color.palette.red-600` | Color | `#dc2626ff`（rgba 220, 38, 38, 255） | (直接値) |
| `color.palette.red-700` | Color | `#b91c1cff`（rgba 185, 28, 28, 255） | (直接値) |
| `color.palette.red-800` | Color | `#991b1bff`（rgba 153, 27, 27, 255） | (直接値) |
| `color.palette.red-900` | Color | `#7f1d1dff`（rgba 127, 29, 29, 255） | (直接値) |
| `color.palette.transparent` | Color | `#00000000`（rgba 0, 0, 0, 0） | (直接値) |
| `color.palette.white` | Color | `#ffffffff`（rgba 255, 255, 255, 255） | (直接値) |
| `color.palette.yellow-100` | Color | `#fef9c3ff`（rgba 254, 249, 195, 255） | (直接値) |
| `color.palette.yellow-200` | Color | `#fef08aff`（rgba 254, 240, 138, 255） | (直接値) |
| `color.palette.yellow-300` | Color | `#fde047ff`（rgba 253, 224, 71, 255） | (直接値) |
| `color.palette.yellow-400` | Color | `#facc15ff`（rgba 250, 204, 21, 255） | (直接値) |
| `color.palette.yellow-50` | Color | `#fefce8ff`（rgba 254, 252, 232, 255） | (直接値) |
| `color.palette.yellow-500` | Color | `#eab308ff`（rgba 234, 179, 8, 255） | (直接値) |
| `color.palette.yellow-600` | Color | `#ca8a04ff`（rgba 202, 138, 4, 255） | (直接値) |
| `color.palette.yellow-700` | Color | `#a16207ff`（rgba 161, 98, 7, 255） | (直接値) |
| `color.palette.yellow-800` | Color | `#854d0eff`（rgba 133, 77, 14, 255） | (直接値) |
| `color.palette.yellow-900` | Color | `#713f12ff`（rgba 113, 63, 18, 255） | (直接値) |
| `color.primary` | Color | → `color.palette.ios-blue`（期待カテゴリ: Color） | `#007affff`（rgba 0, 122, 255, 255） |
| `color.secondary` | Color | → `color.palette.ios-gray`（期待カテゴリ: Color） | `#8e8e93ff`（rgba 142, 142, 147, 255） |
| `color.success` | Color | → `color.palette.ios-green`（期待カテゴリ: Color） | `#34c759ff`（rgba 52, 199, 89, 255） |
| `color.surface` | Color | → `color.palette.ios-surface`（期待カテゴリ: Color） | `#f2f2f7ff`（rgba 242, 242, 247, 255） |
| `color.warning` | Color | → `color.palette.ios-orange`（期待カテゴリ: Color） | `#ff9500ff`（rgba 255, 149, 0, 255） |
| `typography.body` | Typography | family=SF Pro, size=17px, line_height=23.8px, weight=400, style=normal | (直接値) |
| `typography.caption` | Typography | family=SF Pro, size=12px, line_height=16.8px, weight=400, style=normal | (直接値) |
| `typography.display` | Typography | family=SF Pro, size=34px, line_height=47.6px, weight=400, style=normal | (直接値) |
| `typography.headline` | Typography | family=SF Pro, size=28px, line_height=39.2px, weight=400, style=normal | (直接値) |
| `typography.label` | Typography | family=SF Pro, size=13px, line_height=18.2px, weight=400, style=normal | (直接値) |
| `typography.title` | Typography | family=SF Pro, size=22px, line_height=30.8px, weight=400, style=normal | (直接値) |
| `spacing.lg` | Spacing | 24px | (直接値) |
| `spacing.md` | Spacing | 16px | (直接値) |
| `spacing.none` | Spacing | 0px | (直接値) |
| `spacing.sm` | Spacing | 8px | (直接値) |
| `spacing.xl` | Spacing | 32px | (直接値) |
| `spacing.xs` | Spacing | 4px | (直接値) |
| `spacing.xxl` | Spacing | 48px | (直接値) |
| `spacing.xxxl` | Spacing | 64px | (直接値) |
| `size.lg` | Size | 48px | (直接値) |
| `size.md` | Size | 32px | (直接値) |
| `size.sm` | Size | 24px | (直接値) |
| `size.xl` | Size | 64px | (直接値) |
| `size.xs` | Size | 16px | (直接値) |
| `radius.full` | Radius | 9999px | (直接値) |
| `radius.lg` | Radius | 16px | (直接値) |
| `radius.md` | Radius | 10px | (直接値) |
| `radius.none` | Radius | 0px | (直接値) |
| `radius.sm` | Radius | 4px | (直接値) |
| `elevation.flat` | Elevation | level 0 | (直接値) |
| `elevation.floating` | Elevation | level 2 | (直接値) |
| `elevation.modal` | Elevation | level 4 | (直接値) |
| `elevation.overlay` | Elevation | level 3 | (直接値) |
| `elevation.raised` | Elevation | level 1 | (直接値) |
| `elevation.top` | Elevation | level 5 | (直接値) |
| `border.dashed` | Border | width=1px, style=dashed, color=`#d4d4d4ff`（rgba 212, 212, 212, 255）（参照元: `color.palette.gray-300`） | (直接値) |
| `border.default` | Border | width=1px, style=solid, color=`#d4d4d4ff`（rgba 212, 212, 212, 255）（参照元: `color.palette.gray-300`） | (直接値) |
| `border.strong` | Border | width=2px, style=solid, color=`#737373ff`（rgba 115, 115, 115, 255）（参照元: `color.palette.gray-500`） | (直接値) |
| `shadow.lg` | Shadow | offset=(0, 8)px, blur=16px, color=`#000000ff`（rgba 0, 0, 0, 255）（参照元: `color.palette.black`） | (直接値) |
| `shadow.md` | Shadow | offset=(0, 4)px, blur=8px, color=`#000000ff`（rgba 0, 0, 0, 255）（参照元: `color.palette.black`） | (直接値) |
| `shadow.sm` | Shadow | offset=(0, 1)px, blur=2px, color=`#000000ff`（rgba 0, 0, 0, 255）（参照元: `color.palette.black`） | (直接値) |
| `duration.fast` | Duration | 100ms | (直接値) |
| `duration.instant` | Duration | 0ms | (直接値) |
| `duration.normal` | Duration | 250ms | (直接値) |
| `duration.slow` | Duration | 400ms | (直接値) |
| `duration.slower` | Duration | 700ms | (直接値) |
| `animation.emphasized` | Animation | duration=400ms, easing=ease-in-out | (直接値) |
| `animation.enter` | Animation | duration=250ms, easing=ease-out | (直接値) |
| `animation.exit` | Animation | duration=100ms, easing=ease-in | (直接値) |
| `animation.standard` | Animation | duration=250ms, easing=ease-in-out | (直接値) |

## iOS ダーク差分

| トークンパス | カテゴリ | 定義 | 解決後の値 |
|---|---|---|---|
| `color.palette.ios-background` | Color | `#000000ff`（rgba 0, 0, 0, 255） | (直接値) |
| `color.palette.ios-blue` | Color | `#0a84ffff`（rgba 10, 132, 255, 255） | (直接値) |
| `color.palette.ios-green` | Color | `#30d158ff`（rgba 48, 209, 88, 255） | (直接値) |
| `color.palette.ios-label` | Color | `#ffffffff`（rgba 255, 255, 255, 255） | (直接値) |
| `color.palette.ios-orange` | Color | `#ff9f0aff`（rgba 255, 159, 10, 255） | (直接値) |
| `color.palette.ios-red` | Color | `#ff453aff`（rgba 255, 69, 58, 255） | (直接値) |
| `color.palette.ios-surface` | Color | `#1c1c1eff`（rgba 28, 28, 30, 255） | (直接値) |

## Android 基本テーマ（ライト、セマンティックカラーを含む）

| トークンパス | カテゴリ | 定義 | 解決後の値 |
|---|---|---|---|
| `color.background` | Color | → `color.palette.android-background`（期待カテゴリ: Color） | `#fffbfeff`（rgba 255, 251, 254, 255） |
| `color.error` | Color | → `color.palette.android-error`（期待カテゴリ: Color） | `#b3261eff`（rgba 179, 38, 30, 255） |
| `color.on-background` | Color | → `color.palette.android-on-surface`（期待カテゴリ: Color） | `#1c1b1fff`（rgba 28, 27, 31, 255） |
| `color.on-primary` | Color | → `color.palette.android-on-primary`（期待カテゴリ: Color） | `#ffffffff`（rgba 255, 255, 255, 255） |
| `color.on-surface` | Color | → `color.palette.android-on-surface`（期待カテゴリ: Color） | `#1c1b1fff`（rgba 28, 27, 31, 255） |
| `color.palette.android-background` | Color | `#fffbfeff`（rgba 255, 251, 254, 255） | (直接値) |
| `color.palette.android-error` | Color | `#b3261eff`（rgba 179, 38, 30, 255） | (直接値) |
| `color.palette.android-on-primary` | Color | `#ffffffff`（rgba 255, 255, 255, 255） | (直接値) |
| `color.palette.android-on-surface` | Color | `#1c1b1fff`（rgba 28, 27, 31, 255） | (直接値) |
| `color.palette.android-primary` | Color | `#6750a4ff`（rgba 103, 80, 164, 255） | (直接値) |
| `color.palette.android-secondary` | Color | `#625b71ff`（rgba 98, 91, 113, 255） | (直接値) |
| `color.palette.android-success` | Color | `#16a34aff`（rgba 22, 163, 74, 255） | (直接値) |
| `color.palette.android-surface` | Color | `#fffbfeff`（rgba 255, 251, 254, 255） | (直接値) |
| `color.palette.android-warning` | Color | `#ca8a04ff`（rgba 202, 138, 4, 255） | (直接値) |
| `color.palette.black` | Color | `#000000ff`（rgba 0, 0, 0, 255） | (直接値) |
| `color.palette.blue-100` | Color | `#dbeafeff`（rgba 219, 234, 254, 255） | (直接値) |
| `color.palette.blue-200` | Color | `#bfdbfeff`（rgba 191, 219, 254, 255） | (直接値) |
| `color.palette.blue-300` | Color | `#93c5fdff`（rgba 147, 197, 253, 255） | (直接値) |
| `color.palette.blue-400` | Color | `#60a5faff`（rgba 96, 165, 250, 255） | (直接値) |
| `color.palette.blue-50` | Color | `#eff6ffff`（rgba 239, 246, 255, 255） | (直接値) |
| `color.palette.blue-500` | Color | `#3b82f6ff`（rgba 59, 130, 246, 255） | (直接値) |
| `color.palette.blue-600` | Color | `#2563ebff`（rgba 37, 99, 235, 255） | (直接値) |
| `color.palette.blue-700` | Color | `#1d4ed8ff`（rgba 29, 78, 216, 255） | (直接値) |
| `color.palette.blue-800` | Color | `#1e40afff`（rgba 30, 64, 175, 255） | (直接値) |
| `color.palette.blue-900` | Color | `#1e3a8aff`（rgba 30, 58, 138, 255） | (直接値) |
| `color.palette.gray-100` | Color | `#f5f5f5ff`（rgba 245, 245, 245, 255） | (直接値) |
| `color.palette.gray-200` | Color | `#e5e5e5ff`（rgba 229, 229, 229, 255） | (直接値) |
| `color.palette.gray-300` | Color | `#d4d4d4ff`（rgba 212, 212, 212, 255） | (直接値) |
| `color.palette.gray-400` | Color | `#a3a3a3ff`（rgba 163, 163, 163, 255） | (直接値) |
| `color.palette.gray-50` | Color | `#fafafaff`（rgba 250, 250, 250, 255） | (直接値) |
| `color.palette.gray-500` | Color | `#737373ff`（rgba 115, 115, 115, 255） | (直接値) |
| `color.palette.gray-600` | Color | `#525252ff`（rgba 82, 82, 82, 255） | (直接値) |
| `color.palette.gray-700` | Color | `#404040ff`（rgba 64, 64, 64, 255） | (直接値) |
| `color.palette.gray-800` | Color | `#262626ff`（rgba 38, 38, 38, 255） | (直接値) |
| `color.palette.gray-900` | Color | `#171717ff`（rgba 23, 23, 23, 255） | (直接値) |
| `color.palette.green-100` | Color | `#dcfce7ff`（rgba 220, 252, 231, 255） | (直接値) |
| `color.palette.green-200` | Color | `#bbf7d0ff`（rgba 187, 247, 208, 255） | (直接値) |
| `color.palette.green-300` | Color | `#86efacff`（rgba 134, 239, 172, 255） | (直接値) |
| `color.palette.green-400` | Color | `#4ade80ff`（rgba 74, 222, 128, 255） | (直接値) |
| `color.palette.green-50` | Color | `#f0fdf4ff`（rgba 240, 253, 244, 255） | (直接値) |
| `color.palette.green-500` | Color | `#22c55eff`（rgba 34, 197, 94, 255） | (直接値) |
| `color.palette.green-600` | Color | `#16a34aff`（rgba 22, 163, 74, 255） | (直接値) |
| `color.palette.green-700` | Color | `#15803dff`（rgba 21, 128, 61, 255） | (直接値) |
| `color.palette.green-800` | Color | `#166534ff`（rgba 22, 101, 52, 255） | (直接値) |
| `color.palette.green-900` | Color | `#14532dff`（rgba 20, 83, 45, 255） | (直接値) |
| `color.palette.red-100` | Color | `#fee2e2ff`（rgba 254, 226, 226, 255） | (直接値) |
| `color.palette.red-200` | Color | `#fecacaff`（rgba 254, 202, 202, 255） | (直接値) |
| `color.palette.red-300` | Color | `#fca5a5ff`（rgba 252, 165, 165, 255） | (直接値) |
| `color.palette.red-400` | Color | `#f87171ff`（rgba 248, 113, 113, 255） | (直接値) |
| `color.palette.red-50` | Color | `#fef2f2ff`（rgba 254, 242, 242, 255） | (直接値) |
| `color.palette.red-500` | Color | `#ef4444ff`（rgba 239, 68, 68, 255） | (直接値) |
| `color.palette.red-600` | Color | `#dc2626ff`（rgba 220, 38, 38, 255） | (直接値) |
| `color.palette.red-700` | Color | `#b91c1cff`（rgba 185, 28, 28, 255） | (直接値) |
| `color.palette.red-800` | Color | `#991b1bff`（rgba 153, 27, 27, 255） | (直接値) |
| `color.palette.red-900` | Color | `#7f1d1dff`（rgba 127, 29, 29, 255） | (直接値) |
| `color.palette.transparent` | Color | `#00000000`（rgba 0, 0, 0, 0） | (直接値) |
| `color.palette.white` | Color | `#ffffffff`（rgba 255, 255, 255, 255） | (直接値) |
| `color.palette.yellow-100` | Color | `#fef9c3ff`（rgba 254, 249, 195, 255） | (直接値) |
| `color.palette.yellow-200` | Color | `#fef08aff`（rgba 254, 240, 138, 255） | (直接値) |
| `color.palette.yellow-300` | Color | `#fde047ff`（rgba 253, 224, 71, 255） | (直接値) |
| `color.palette.yellow-400` | Color | `#facc15ff`（rgba 250, 204, 21, 255） | (直接値) |
| `color.palette.yellow-50` | Color | `#fefce8ff`（rgba 254, 252, 232, 255） | (直接値) |
| `color.palette.yellow-500` | Color | `#eab308ff`（rgba 234, 179, 8, 255） | (直接値) |
| `color.palette.yellow-600` | Color | `#ca8a04ff`（rgba 202, 138, 4, 255） | (直接値) |
| `color.palette.yellow-700` | Color | `#a16207ff`（rgba 161, 98, 7, 255） | (直接値) |
| `color.palette.yellow-800` | Color | `#854d0eff`（rgba 133, 77, 14, 255） | (直接値) |
| `color.palette.yellow-900` | Color | `#713f12ff`（rgba 113, 63, 18, 255） | (直接値) |
| `color.primary` | Color | → `color.palette.android-primary`（期待カテゴリ: Color） | `#6750a4ff`（rgba 103, 80, 164, 255） |
| `color.secondary` | Color | → `color.palette.android-secondary`（期待カテゴリ: Color） | `#625b71ff`（rgba 98, 91, 113, 255） |
| `color.success` | Color | → `color.palette.android-success`（期待カテゴリ: Color） | `#16a34aff`（rgba 22, 163, 74, 255） |
| `color.surface` | Color | → `color.palette.android-surface`（期待カテゴリ: Color） | `#fffbfeff`（rgba 255, 251, 254, 255） |
| `color.warning` | Color | → `color.palette.android-warning`（期待カテゴリ: Color） | `#ca8a04ff`（rgba 202, 138, 4, 255） |
| `typography.body` | Typography | family=Roboto, size=16px, line_height=22.4px, weight=400, style=normal | (直接値) |
| `typography.caption` | Typography | family=Roboto, size=12px, line_height=16.8px, weight=400, style=normal | (直接値) |
| `typography.display` | Typography | family=Roboto, size=36px, line_height=50.4px, weight=400, style=normal | (直接値) |
| `typography.headline` | Typography | family=Roboto, size=28px, line_height=39.2px, weight=400, style=normal | (直接値) |
| `typography.label` | Typography | family=Roboto, size=14px, line_height=19.6px, weight=400, style=normal | (直接値) |
| `typography.title` | Typography | family=Roboto, size=22px, line_height=30.8px, weight=400, style=normal | (直接値) |
| `spacing.lg` | Spacing | 24px | (直接値) |
| `spacing.md` | Spacing | 16px | (直接値) |
| `spacing.none` | Spacing | 0px | (直接値) |
| `spacing.sm` | Spacing | 8px | (直接値) |
| `spacing.xl` | Spacing | 32px | (直接値) |
| `spacing.xs` | Spacing | 4px | (直接値) |
| `spacing.xxl` | Spacing | 48px | (直接値) |
| `spacing.xxxl` | Spacing | 64px | (直接値) |
| `size.lg` | Size | 48px | (直接値) |
| `size.md` | Size | 32px | (直接値) |
| `size.sm` | Size | 24px | (直接値) |
| `size.xl` | Size | 64px | (直接値) |
| `size.xs` | Size | 16px | (直接値) |
| `radius.full` | Radius | 9999px | (直接値) |
| `radius.lg` | Radius | 16px | (直接値) |
| `radius.md` | Radius | 12px | (直接値) |
| `radius.none` | Radius | 0px | (直接値) |
| `radius.sm` | Radius | 4px | (直接値) |
| `elevation.flat` | Elevation | level 0 | (直接値) |
| `elevation.floating` | Elevation | level 2 | (直接値) |
| `elevation.modal` | Elevation | level 4 | (直接値) |
| `elevation.overlay` | Elevation | level 3 | (直接値) |
| `elevation.raised` | Elevation | level 1 | (直接値) |
| `elevation.top` | Elevation | level 5 | (直接値) |
| `border.dashed` | Border | width=1px, style=dashed, color=`#d4d4d4ff`（rgba 212, 212, 212, 255）（参照元: `color.palette.gray-300`） | (直接値) |
| `border.default` | Border | width=1px, style=solid, color=`#d4d4d4ff`（rgba 212, 212, 212, 255）（参照元: `color.palette.gray-300`） | (直接値) |
| `border.strong` | Border | width=2px, style=solid, color=`#737373ff`（rgba 115, 115, 115, 255）（参照元: `color.palette.gray-500`） | (直接値) |
| `shadow.lg` | Shadow | offset=(0, 8)px, blur=16px, color=`#000000ff`（rgba 0, 0, 0, 255）（参照元: `color.palette.black`） | (直接値) |
| `shadow.md` | Shadow | offset=(0, 4)px, blur=8px, color=`#000000ff`（rgba 0, 0, 0, 255）（参照元: `color.palette.black`） | (直接値) |
| `shadow.sm` | Shadow | offset=(0, 1)px, blur=2px, color=`#000000ff`（rgba 0, 0, 0, 255）（参照元: `color.palette.black`） | (直接値) |
| `duration.fast` | Duration | 100ms | (直接値) |
| `duration.instant` | Duration | 0ms | (直接値) |
| `duration.normal` | Duration | 300ms | (直接値) |
| `duration.slow` | Duration | 400ms | (直接値) |
| `duration.slower` | Duration | 700ms | (直接値) |
| `animation.emphasized` | Animation | duration=400ms, easing=ease-in-out | (直接値) |
| `animation.enter` | Animation | duration=300ms, easing=ease-out | (直接値) |
| `animation.exit` | Animation | duration=100ms, easing=ease-in | (直接値) |
| `animation.standard` | Animation | duration=300ms, easing=ease-in-out | (直接値) |

## Android ダーク差分

| トークンパス | カテゴリ | 定義 | 解決後の値 |
|---|---|---|---|
| `color.palette.android-background` | Color | `#1c1b1fff`（rgba 28, 27, 31, 255） | (直接値) |
| `color.palette.android-error` | Color | `#f2b8b5ff`（rgba 242, 184, 181, 255） | (直接値) |
| `color.palette.android-on-primary` | Color | `#381e72ff`（rgba 56, 30, 114, 255） | (直接値) |
| `color.palette.android-on-surface` | Color | `#e6e1e5ff`（rgba 230, 225, 229, 255） | (直接値) |
| `color.palette.android-primary` | Color | `#d0bcffff`（rgba 208, 188, 255, 255） | (直接値) |
| `color.palette.android-secondary` | Color | `#ccc2dcff`（rgba 204, 194, 220, 255） | (直接値) |
| `color.palette.android-success` | Color | `#4ade80ff`（rgba 74, 222, 128, 255） | (直接値) |
| `color.palette.android-surface` | Color | `#1c1b1fff`（rgba 28, 27, 31, 255） | (直接値) |
| `color.palette.android-warning` | Color | `#facc15ff`（rgba 250, 204, 21, 255） | (直接値) |
