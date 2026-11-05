// 互換用: パッケージ移動前（com.fandhe.androidapp）にユーザーが追加済みのクイック設定タイルは、
// OS が旧完全修飾名の service を保持している。旧名を残して新クラスへ委譲し、タップを受け続けられるようにする。
package com.fandhe.androidapp;

public final class QuickSettingsTileService extends com.fandhe.platform.QuickSettingsTileService {}
