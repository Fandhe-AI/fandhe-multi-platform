#!/usr/bin/env bash
# CI の `cargo test` job で GNU ld が desktop_render（fandhe-egui-proxy、GUI 依存の
# 大量 rlib を静的リンクするテストバイナリ）のリンク時に OOM killer に kill される
# 事象（親 Issue #630）を、rust-toolchain.toml 固定の rustc 1.88.0 に同梱される
# rust-lld（`<sysroot>/lib/rustlib/<host>/bin/gcc-ld/ld.lld`）へ切り替えて回避する
# ための RUSTFLAGS 算出・検証スクリプト（Issue #631）。
#
# system lld / clang は追加導入しない。toolchain 同梱の rust-lld なら追加インストール
# 不要かつ toolchain と同じ供給元でバージョンが固定される。
#
# 重要な落とし穴（実測で確認済み）: `-C link-arg=-fuse-ld=lld` を単独で渡すと、
# lld が見つからない環境では collect2 が `[cannot find ld]` を出しつつ
# GNU ld へ黙って fallback してリンクが成功してしまう。「ビルドが通った」ことは
# lld 使用の証拠にならないため、`--verify-link` モードで fail-closed に機械検証する。
#
# 使い方（ローカル）:
#   RUSTFLAGS="$(bash scripts/build/lld-rustflags.sh)" cargo test -p fandhe-egui-proxy --test desktop_render --no-run
#
# 使い方（CI・.github/workflows/ci.yml の test job から呼ばれる想定）:
#   bash scripts/build/lld-rustflags.sh            # RUSTFLAGS の値を stdout へ出力
#   bash scripts/build/lld-rustflags.sh --verify-link  # lld が実際に使われることを検証
#
# 将来的な撤去について: rustc 1.90 以降は x86_64-unknown-linux-gnu の既定リンカが
# lld になる予定のため、ランナーの toolchain を 1.90 以上へ上げた時点で本スクリプトと
# ci.yml 側の呼び出しステップは撤去してよい。
set -euo pipefail

mode="${1:-}"
if [[ -n "${mode}" && "${mode}" != "--verify-link" ]]; then
  echo "[lld-rustflags] エラー: 不明な引数: ${mode}（使用可能: --verify-link のみ）" >&2
  exit 1
fi

# --- 共通ガード: Linux gnu ホスト限定 -----------------------------------
# macOS（開発機）では
# gcc-ld/ld.lld レイアウトが異なる（ld64.lld を使う）ため対象外とする。
#
# デフォルトモードでは何も出力せず exit 0 しておくことで、呼び出し側（ci.yml）で
# `RUSTFLAGS=$(...)` が空文字になり no-op として扱える。
# 一方 `--verify-link` モードでここを no-op（exit 0）にすると、ci.yml 側の
# `runner.os == 'Linux'` ガードが gnu かどうかまでは区別しないため、万一
# 非 gnu の Linux runner（musl 等）が割り当たった際に「検証ステップが何も検証
# せず緑になる」フェイル・オープンを招く（fail-closed 設計の趣旨に反する）。
# そのため `--verify-link` では非 gnu ホストを明示的な失敗として扱う。
host="$(rustc -vV | sed -n 's/^host: //p')"
if [[ "${host}" != *-unknown-linux-gnu ]]; then
  if [[ "${mode}" == "--verify-link" ]]; then
    echo "[lld-rustflags] エラー: Linux gnu ホストではない（host=${host}）ため --verify-link を実行できません（fail-closed）" >&2
    exit 1
  fi
  echo "[lld-rustflags] notice: Linux gnu ホストではない（host=${host}）ため lld 化をスキップします" >&2
  exit 0
fi

sysroot="$(rustc --print sysroot)"
gccld="${sysroot}/lib/rustlib/${host}/bin/gcc-ld"

# RUSTFLAGS はスペース区切りでフラグを連結するため、パスに空白が含まれると
# 意図しない分割を招く。安全側で拒否する（fail-closed）。
if [[ "${gccld}" == *' '* ]]; then
  echo "[lld-rustflags] エラー: gcc-ld のパスに空白が含まれるため安全に RUSTFLAGS へ渡せない: ${gccld}" >&2
  exit 1
fi

lld_bin="${gccld}/ld.lld"
if [[ ! -x "${lld_bin}" ]]; then
  echo "[lld-rustflags] エラー: rust-lld が見つからない（toolchain 同梱の想定パスに実行可能ファイルがない）: ${lld_bin}" >&2
  exit 1
fi

lld_version="$("${lld_bin}" --version)"
if [[ "${lld_version}" != *LLD* ]]; then
  echo "[lld-rustflags] エラー: ${lld_bin} --version の出力に 'LLD' が含まれない: ${lld_version}" >&2
  exit 1
fi

# collect2 の GNU ld silent fallback（-fuse-ld=lld 指定時に lld が見つからないと
# 黙って GNU ld へ fallback してビルド成功してしまう）を、cc ドライバに実際に
# 選ばせて機械的に排除する。`-print-prog-name=ld.lld` は「このドライバ設定で
# ld.lld として解決される実行ファイルパス」を返すため、期待パスと一致しなければ
# 我々が渡す -B が効いていない（＝別の ld.lld か GNU ld へフォールバックし得る）。
resolved="$(cc -B"${gccld}" -print-prog-name=ld.lld)"
if [[ "${resolved}" != "${lld_bin}" ]]; then
  echo "[lld-rustflags] エラー: cc -B${gccld} -print-prog-name=ld.lld の解決結果が期待パスと不一致: resolved=${resolved} expected=${lld_bin}" >&2
  exit 1
fi

if [[ "${mode}" == "--verify-link" ]]; then
  # rustc 経由のリンクではリンカの stdout が成功時に握り潰されるため、検証は
  # rustc が cc に渡すのと同一形（`-B<gcc-ld dir>` + `-fuse-ld=lld`）で cc
  # ドライバを直叩きし、collect2 が実際に起動したリンカの絶対パスを確認する。
  # `-Wl,--version` を linker へ渡すと collect2 は（`-v` なしでも）実行する
  # リンカコマンドラインをそのまま stderr へ echo する（ローカル実測で確認済み）。
  # このログに ${lld_bin} の絶対パスが現れるかどうかで判別できる:
  # `-B` が効いていれば toolchain 同梱の ld.lld のフルパスが、silent fallback
  # していれば `/usr/bin/ld`（GNU ld）が現れる（`-B` を外した比較実測で確認済み、
  # ネガティブケースで grep が確実に不一致になることを別途検証済み）。
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "${tmpdir}"' EXIT

  probe_src="${tmpdir}/probe.c"
  probe_out="${tmpdir}/probe"
  collect2_log="${tmpdir}/collect2-invocation.log"
  cat >"${probe_src}" <<'EOF'
int main(void) { return 0; }
EOF

  if ! cc -B"${gccld}" -fuse-ld=lld -Wl,--version "${probe_src}" -o "${probe_out}" >"${collect2_log}" 2>&1; then
    echo "[lld-rustflags] エラー: 検証用リンクに失敗しました。ログ:" >&2
    cat "${collect2_log}" >&2
    exit 1
  fi

  if ! grep -qF "${lld_bin}" "${collect2_log}"; then
    echo "[lld-rustflags] エラー: 検証用リンクのログに期待した ld.lld の絶対パスが現れませんでした（GNU ld へ fallback した疑い）" >&2
    echo "[lld-rustflags] 期待パス: ${lld_bin}" >&2
    echo "[lld-rustflags] ログ:" >&2
    cat "${collect2_log}" >&2
    exit 1
  fi

  echo "[lld-rustflags] 検証 OK: lld が実際にリンクへ使われています"
  echo "[lld-rustflags] リンカパス: ${lld_bin}"
  echo "[lld-rustflags] バージョン: ${lld_version}"
  exit 0
fi

# --- デフォルトモード: RUSTFLAGS の値を stdout へ出力 ---------------------
echo "-Clink-arg=-B${gccld} -Clink-arg=-fuse-ld=lld"
