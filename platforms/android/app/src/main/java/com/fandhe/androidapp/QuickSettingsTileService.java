package com.fandhe.androidapp;

import android.service.quicksettings.TileService;

// クイック設定タイル（Issue #720、親 #139）の `TileService`。AndroidManifest の `<service>`
// （exported=true、`BIND_QUICK_SETTINGS_TILE` 要求、intent-filter は `QS_TILE`）で宣言され、
// bind できるのはシステムだけ。タイルの追加はユーザーがクイック設定の編集から行う。
// 非アクティブモード（ACTIVE_TILE の meta-data なし）で、タイル側への回数表示はしない。
// タップのたびに `QuickSettingsTileStore.increment` で回数を 1 増やし、アプリ画面が
// Rust（`android_quick_settings_tile.rs`）経由で読む。アプリのプロセスが無ければ OS がこの
// サービスのために起こすが、MainActivity は読み込まれない。切り離し時は Manifest の宣言ごと削除する。
public final class QuickSettingsTileService extends TileService {
    @Override
    public void onClick() {
        super.onClick();
        QuickSettingsTileStore.increment(getApplicationContext());
    }
}
