package com.fandhe.platform;

import androidx.core.content.FileProvider;

// video-capture 専用の FileProvider（Issue #1192）。動画の撮影が撮影アプリへ渡す書き込み先の content URI の発行だけを担う。
// 共有の FileProvider（`${applicationId}.compose`）を使うと、メール添付など他機能の公開パスと同じ設定ファイルに
// 載ってしまうため、機能 crate が自分だけで完結するよう派生を持つ。宣言は `fandhe-cap-video-capture` の Cargo.toml
// （`android.components`）と AndroidManifest.xml にあり、公開するのは `cache/video_capture/` だけ。
public final class VideoCaptureFileProvider extends FileProvider {}
