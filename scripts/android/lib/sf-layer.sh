#!/usr/bin/env bash
# SurfaceFlinger レイヤ選択の共通ロジック（Issue #658 P1 レビュー対応）。
#
# measure-frame-drop-rate.sh の find_layer と measure-frame-rate.sh フェーズ 7 が
# それぞれ独自にレイヤ選択を実装すると、SurfaceView 優先規則がずれて別レイヤの
# presentation 間隔を拾い得る（codex P1 / Cursor Medium 指摘）。両方から本関数を
# source して使う。
#
# 引数: $1 = `dumpsys SurfaceFlinger --list` の生出力（改行区切り）
#       $2 = 対象パッケージ名
# 標準出力: 選択したレイヤ名（1 行、改行なし）。
# 戻り値: 0 = 選択成功 / 1 = パッケージに一致するレイヤなし / 2 = レイヤ名に想定外の文字を含む
#        / 3 = 同一優先度で複数レイヤが候補になり一意に選べない（fail-closed）
select_surfaceflinger_layer() {
  local list="$1" pkg="$2" escaped_pkg pattern sv_matches fallback_matches layer count

  # grep -F の単純部分一致では `com.example.app` が `com.example.app.debug` や
  # `com.example.application` にも一致し、別アプリのレイヤを誤って選びうる
  # （codex P1 レビュー指摘、Issue #658）。パッケージ名を正規表現のリテラルとして
  # エスケープした上で、前後が識別子文字（英数字・アンダースコア・ドット）で
  # 続かないことを要求し、SurfaceFlinger のレイヤ表記上のパッケージ境界
  # （`/` `]` `#` 空白等）まで検証して一意に選択する。
  escaped_pkg="$(printf '%s' "${pkg}" | sed 's/[.[\*^$/]/\\&/g')"
  pattern="(^|[^A-Za-z0-9_.])${escaped_pkg}([^A-Za-z0-9_.]|\$)"

  # SurfaceView レイヤを優先する（fandhe / Flutter 等の native surface はこちらに現れる）。
  # Android は SurfaceView ごとに "Background for SurfaceView[...]" というコンパニオン
  # レイヤ（Z 順で SurfaceView の背後に置かれる不透明色埋めレイヤ）を追加で生成する。
  # これも `SurfaceView` にマッチしてしまい、フォアグラウンド SurfaceView 1 つに対して
  # 2 レイヤがヒットして一意性チェック（複数候補 fail-closed）を誤って発火させる
  # （Cursor Bugbot High 指摘、Issue #658）。コンパニオンレイヤを除外し、
  # 実際の SurfaceView（presentation 間隔の計測対象）だけを残す。
  sv_matches="$(printf '%s\n' "${list}" | grep -E "${pattern}" | grep -i 'SurfaceView' | grep -vi 'Background for' || true)"
  if [ -n "${sv_matches}" ]; then
    count="$(printf '%s\n' "${sv_matches}" | grep -c '')"
    [ "${count}" -eq 1 ] || return 3
    layer="${sv_matches}"
  else
    fallback_matches="$(printf '%s\n' "${list}" | grep -E "${pattern}" || true)"
    [ -n "${fallback_matches}" ] || return 1
    count="$(printf '%s\n' "${fallback_matches}" | grep -c '')"
    # 複数候補が残る場合、無条件に先頭行を選ぶと背景/Activity ウィンドウ等の
    # 別レイヤを誤って presentation 計測対象にしうるため、一意に絞れないとして
    # fail-closed にする（P1 レビュー指摘、Issue #658）。
    [ "${count}" -eq 1 ] || return 3
    layer="${fallback_matches}"
  fi

  # レイヤ名は未信頼値。SurfaceView の標準的な表記（`SurfaceView[pkg/Activity]#0` 等の
  # 角括弧・丸括弧）を許可しつつ、それ以外の想定外文字（シェルメタ文字等）を
  # 含む場合は fail-closed とする。
  [[ "${layer}" =~ ^[]A-Za-z0-9._#/()[\ -]+$ ]] || return 2
  printf '%s' "${layer}"
}
