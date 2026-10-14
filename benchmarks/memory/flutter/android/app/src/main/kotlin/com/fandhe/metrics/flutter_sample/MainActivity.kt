package com.fandhe.metrics.flutter_sample

import io.flutter.embedding.android.FlutterActivity

/**
 * Flutter エンジンを起動するだけの計測用エントリポイント。
 * S3（アイドル時メモリ計測、Issue #658）から
 * `adb shell am start -n com.fandhe.metrics.flutter_sample/.MainActivity` で起動される契約。
 */
class MainActivity : FlutterActivity()
