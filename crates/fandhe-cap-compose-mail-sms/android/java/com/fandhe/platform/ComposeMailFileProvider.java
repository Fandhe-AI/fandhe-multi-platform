package com.fandhe.platform;

import androidx.core.content.FileProvider;

// compose-mail-sms 専用の FileProvider（Issue #1193）。メール添付の content URI の発行だけを担う。
// 共有の FileProvider（`${applicationId}.compose`）を使うと、動画の撮影の公開パスと同じ設定ファイルに載ってしまうため、
// 機能 crate が自分だけで完結するよう派生を持つ。宣言は `fandhe-cap-compose-mail-sms` の Cargo.toml
// （`android.components`）と AndroidManifest.xml にあり、公開するのは `cache/compose/` だけ。
public final class ComposeMailFileProvider extends FileProvider {}
