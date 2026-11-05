// 互換用: パッケージ移動前（com.fandhe.androidapp）に OS へ予約済みのアラームは、旧完全修飾名の
// receiver 宛ての PendingIntent を保持している。旧名を残して新クラスへ委譲し、鳴らせるようにする。
// 新規予約は com.fandhe.platform 側を使う（AlarmHelper が旧予約の照合・取消を行う）。
package com.fandhe.androidapp;

public final class AlarmReceiver extends com.fandhe.platform.AlarmReceiver {}
