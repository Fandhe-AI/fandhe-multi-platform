package com.fandhe.platform;

import androidx.core.content.FileProvider;

// file-preview 専用の FileProvider（Issue #1192）。ファイルのプレビューが他アプリへ渡す content URI の発行だけを担う。
// 共有の FileProvider（`${applicationId}.compose`）を使うと、メール添付・動画の撮影の公開パスと同じ設定ファイルに
// 載ってしまうため、機能 crate が自分だけで完結するよう派生を持つ。宣言は `fandhe-cap-file-preview` の Cargo.toml
// （`android.components`）と AndroidManifest.xml にあり、公開するのは `cache/file_preview/` だけ。
public final class FilePreviewFileProvider extends FileProvider {}
