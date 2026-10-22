# 開発環境整備（REQ-18）: 共通コア（crates/fandhe-core 等）・Linux・Web（wasm32）向けの
# 開発用コンテナイメージ。
#
# 対象外: Android NDK / Apple（Xcode）系のクロスビルド環境はこのイメージに含めない。
# Android は scripts/android/setup-android-env.sh によるホスト側（Linux/macOS）導入を
# 前提とし、Apple 系（iOS/watchOS/visionOS/macOS）は Xcode 依存のため Docker 化しない
# （scripts/apple/・scripts/ios/ 配下スクリプトはホスト macOS 上での実行が前提）。
#
# rust-toolchain.toml が channel = "1.99.1" を固定しているため、ベースイメージのタグも
# 同一バージョンに固定する（ズレるとコンテナ内で rustup が別バージョンの取得を試み、
# CI ランナー（.claude/rules/ci.md）との再現性が崩れるため）。
FROM rust:1.99.1-slim-bookworm

# 公式 rust イメージは CARGO_HOME=/usr/local/cargo・RUSTUP_HOME=/usr/local/rustup を
# 既定で設定済み。CARGO_TARGET_DIR は明示的に上書きしない（cargo 既定の
# <repo>/target = /work/target のまま使う）。理由: scripts/web/build-wasm-proxy.sh
# （TASK-1.2）が wasm 成果物パスを `${repo_root}/target/wasm32-unknown-unknown/...` に
# 決め打ちしており CARGO_TARGET_DIR を参照しないため、/cargo-target 等へ逃がすと
# `make wasm` 相当のスクリプトが成果物を見つけられず失敗する（Cursor Bugbot 指摘・
# PR #512 是正）。/work/target は bind mount する /work の一部としてホスト側に
# 実体を持つため、named volume 化せずとも自然に永続化される
# （/target は .gitignore 済みでコミット対象にもならない）。
#
# Linux 向け GUI 検証（scripts/metrics/measure-idle-memory.sh 等が Xvfb を使う）と
# ネイティブ依存クレートのビルドに必要な最小限のパッケージのみを追加する。
# - pkg-config: ネイティブ依存クレートのビルド探索に必要
# - xvfb: ヘッドレス環境での GUI 起動検証（idle memory 計測等）に必要
# - ca-certificates: cargo のレジストリ通信（HTTPS）に必要
# - procps: measure-idle-memory.sh が RSS サンプリングに使う `ps`（`-o rss=`/`-o state=`）
#   の提供元。slim イメージには既定で入っておらず、未導入だと前提コマンド検査で
#   即失敗することを Issue #513 検証で確認した
# - libxcursor1 / libxi6 / libxkbcommon-x11-0: fandhe-egui-proxy（eframe の
#   "x11" feature、native ターゲット分岐）が winit 経由で dlopen する実行時共有
#   ライブラリ。xvfb パッケージ自身の依存では libx11-6・libgl1・libglx0・
#   libgl1-mesa-dri までしか入らず、この 3 つは別途必要になることを Xvfb 上での
#   実起動検証で確認した（`libXcursor.so.1` → `libXi.so.6` →
#   `libxkbcommon-x11.so` の順に欠落エラーが発生し、追加後に起動成功。
#   Cursor Bugbot 指摘・PR #512 是正、docs/spec 外の実機検証で確認済み）。
#   ビルド（`cargo build`）自体はこれらのライブラリなしでも成功する
#   （x11-dl/xkbcommon-dl は dlopen 方式で実行時にのみロードするため）。
#   Wayland 実行時ライブラリ（libwayland-client 等）は Xvfb（X11 専用）環境では
#   不要と確認済みのため追加しない（winit は dlopen 失敗時に X11 backend へ
#   自動フォールバックする）。
# - git: scripts/metrics/measure-code-ratio.sh 等が `git rev-parse` 等でコミット情報を
#   参照する。未導入だとこれらのスクリプトがコンテナ内で失敗する
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config \
        xvfb \
        ca-certificates \
        procps \
        git \
        libxcursor1 \
        libxi6 \
        libxkbcommon-x11-0 \
    && rm -rf /var/lib/apt/lists/*

# rust-toolchain.toml の components（rustfmt・clippy）と Web ターゲット（wasm32）を
# 非 root ユーザーへの切り替え前（root）に導入する。
RUN rustup component add rustfmt clippy \
    && rustup target add wasm32-unknown-unknown

# 非 root 実行（最小権限）。CARGO_HOME・RUSTUP_HOME は非 root ユーザーが
# 書き込めるよう所有権を移してから USER を切り替える。
#
# CARGO_HOME/registry は compose.yaml が named volume（cargo-registry。実体の volume 名は
# `fandhe-cargo-registry`。compose.yaml の volumes.cargo-registry.name で明示指定しており、
# compose 管理外のプロセスからも同一 volume を参照できる）を
# マウントする対象だが、ベースイメージにはこのサブディレクトリ自体が
# 存在しない（初回 cargo 実行時に遅延作成される）。存在しないパスへ named
# volume をマウントすると Docker が root:root 755 のマウントポイントを新規作成し、
# 非 root ユーザーが書き込めず `cargo build`/`cargo fetch` が
# `Permission denied` で失敗することを実機確認した（Cursor Bugbot 指摘・
# PR #512 是正）。あらかじめ `mkdir -p` してから chown することで、volume
# マウント時に既存ディレクトリの所有権（fandhe:fandhe）が volume 側へ
# 引き継がれるようにする。
#
# コンテナ内ユーザーの UID/GID をビルド引数化する（Cursor Bugbot 指摘・
# PR #512 コミット 9d5af38 是正）。CARGO_TARGET_DIR を /work/target（bind mount
# された /work の一部）に戻したことで、Linux ホストでは cargo が bind mount 先へ
# 直接書き込むようになった。コンテナ内ユーザーの UID が固定（旧: 10001）だと、
# 多くの Linux ホストのリポジトリ所有者（一般的な初期ユーザーの uid/gid である
# 1000）と一致せず、`docker compose run` 経由の `cargo build` が
# `Permission denied` になることを Linux 環境相当の検証で確認した
# （macOS Docker Desktop はファイル共有層がホスト所有権を透過的に扱うため
# 顕在化しない）。既定値 1000 は多くの Linux デスクトップの初期ユーザーと一致し、
# 固定 10001 より安全側に倒れる。compose.yaml の build.args・Makefile の
# HOST_UID/HOST_GID（`id -u`/`id -g` 自動検出）経由でホストの実際の値へ
# 上書きできる。
ARG UID=1000
ARG GID=1000

# --force（groupadd）・--non-unique（useradd）は、ホストの GID/UID がベースイメージの
# 既存システムアカウントと衝突するケースに対する保険。例えば macOS の既定 GID
# （staff = 20）は Debian では "dialout" グループの GID と一致しており、素の
# groupadd はこの衝突で失敗することを実機確認した。以後の chown・USER 切り替えは
# グループ名ではなく数値 UID/GID で行うため、衝突時にグループ名が "fandhe" に
# ならなくても（例: 既存の "dialout" を再利用しても）動作に支障はない。
RUN groupadd --force --gid "${GID}" fandhe \
    && useradd --non-unique --create-home --uid "${UID}" --gid "${GID}" --shell /bin/bash fandhe \
    && mkdir -p "${CARGO_HOME}/registry" \
    && chown -R "${UID}:${GID}" "${CARGO_HOME}" "${RUSTUP_HOME}"

# X サーバー（Xvfb）は /tmp/.X11-unix が root 所有・sticky bit 付き 1777 であることを
# 起動時に検査する仕様で、なければ `_XSERVTransmkdir` エラーで起動失敗する。非 root
# ユーザー（fandhe）実行のまま満たすため、USER 切り替え前（root）にビルド時点で
# 作成しておく（Issue #513）。
RUN install -d -m 1777 -o root -g root /tmp/.X11-unix

USER fandhe
WORKDIR /work
