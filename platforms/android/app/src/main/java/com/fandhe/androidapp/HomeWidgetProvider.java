// 互換用: パッケージ移動前（com.fandhe.androidapp）に配置済みのホーム画面ウィジェットは、
// OS が旧完全修飾名の receiver を保持している。旧名を残して新クラスへ委譲し、更新を受け続けられるようにする。
// 新規配置は com.fandhe.platform 側が使われる。
package com.fandhe.androidapp;

public final class HomeWidgetProvider extends com.fandhe.platform.HomeWidgetProvider {}
