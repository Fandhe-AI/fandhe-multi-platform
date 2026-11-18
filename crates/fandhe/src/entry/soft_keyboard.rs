//! ソフトキーボードの状態バッファ差分を egui の入力イベント列へ変換する純関数群
//! （Issue #796 Android / #795 iOS）。
//!
//! Android の `AndroidApp::text_input_state()`（`TextInputState { text, compose_region }`）・
//! iOS の `UITextField.text` + `markedTextRange` は、いずれも「確定済みテキスト全体
//! （未確定文字列を含む 1 本の文字列）＋ 未確定領域のスパン」という同じ形をしている。
//! この共通形を `(text: &str, compose: Option<(usize, usize)>)` として本モジュールへ渡すことで、
//! `android.rs`（#796）・`ios.rs`（#795）の両ポーリング実装がプラットフォーム型
//! （`winit::platform::android::activity::TextInputState` / `NSRange` 等）を一切
//! 引きずらずに同じ変換ロジックを共有できる。呼び出し元がプラットフォーム固有の単位
//! （Android は Java の UTF-16 コード単位、iOS は `NSString` の UTF-16 コード単位）を
//! そのまま `compose` に渡せるよう、[`utf16_to_byte_index`] で UTF-8 へ変換する。

/// 状態バッファ 1 回分を単位変換済みで持つ（`text` は UTF-8、未確定文字列は
/// 文字列として切り出し済み）。[`snapshot`] が構築する。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct KeyboardSnapshot {
    /// 確定済みテキスト（未確定文字列を除いた残り）。
    pub committed: String,
    /// 未確定（IME 変換中）文字列。`None` なら未確定領域なし。
    pub compose: Option<String>,
}

/// UTF-16 コード単位のインデックスを、同じ文字列上の UTF-8 バイトインデックスへ変換する。
///
/// Android の `TextSpan` / iOS の `NSRange` はいずれも UTF-16 コード単位でオフセットを
/// 表す（Java `String` / `NSString` の内部表現）ため、Rust の `&str`（UTF-8）上での
/// スライスに使う前にこの変換が要る。`utf16_index` が文字列の UTF-16 長を超える場合、
/// またはサロゲートペア（絵文字等）の途中を指す場合は `None` を返す（壊れた span を
/// そのまま丸めて別の位置を指してしまうことを防ぐ。呼び出し元は [`snapshot`] のとおり
/// `None` を「未確定領域なし」へフォールバックさせる）。ちょうど文字列末尾（UTF-16 長と
/// 一致する位置）は `Some(text.len())` を返す。
pub(crate) fn utf16_to_byte_index(text: &str, utf16_index: usize) -> Option<usize> {
    let mut utf16_pos = 0usize;
    for (byte_idx, ch) in text.char_indices() {
        if utf16_pos == utf16_index {
            return Some(byte_idx);
        }
        if utf16_pos > utf16_index {
            // 直前の文字（サロゲートペア = len_utf16() == 2）の途中を指している。
            return None;
        }
        utf16_pos += ch.len_utf16();
    }
    (utf16_pos == utf16_index).then_some(text.len())
}

/// 状態バッファ（`text` 全体と、未確定領域の UTF-16 span）から [`KeyboardSnapshot`] を作る。
///
/// `compose` の span が壊れている（範囲外・サロゲートペアの途中を指す・空幅）場合は
/// 未確定領域なし（`compose: None`）として扱う（両端が [`utf16_to_byte_index`] で
/// `Some` に解決でき、かつ `start < end` の場合のみ未確定領域として扱う）。空幅の扱いは
/// android-activity `TextInputState::compose_region` の契約（「結果が空幅なら None と
/// 等価」）に合わせている。
///
/// `compose` の座標系は UTF-16 コード単位（Bugbot 指摘の「UTF-8 バイト位置」ではない）。
/// GameTextInput のネイティブ側 `stateFromJava`（`gametextinput.cpp`）が Java `State`
/// （`selectionStart`/`selectionEnd`/`composingRegionStart`/`composingRegionEnd`、いずれも
/// `String`（UTF-16）上のインデックス）の int フィールドを単位変換せずそのまま
/// `TextInputState`/`TextSpan` へ格納するため。
pub(crate) fn snapshot(text: &str, compose: Option<(usize, usize)>) -> KeyboardSnapshot {
    let Some((start, end)) = compose else {
        return KeyboardSnapshot {
            committed: normalize_newlines(text),
            compose: None,
        };
    };
    let (start, end) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };
    // UTF-16 オフセットは正規化前の `text` に対して解決する（`\r\n` → `\n` の正規化で
    // 1 コード単位分ずれると span がずれるため、切り出しが終わってから正規化する）。
    let (Some(byte_start), Some(byte_end)) = (
        utf16_to_byte_index(text, start),
        utf16_to_byte_index(text, end),
    ) else {
        return KeyboardSnapshot {
            committed: normalize_newlines(text),
            compose: None,
        };
    };
    if byte_start >= byte_end {
        return KeyboardSnapshot {
            committed: normalize_newlines(text),
            compose: None,
        };
    }
    let mut committed = String::with_capacity(text.len() - (byte_end - byte_start));
    committed.push_str(&text[..byte_start]);
    committed.push_str(&text[byte_end..]);
    KeyboardSnapshot {
        committed: normalize_newlines(&committed),
        compose: Some(normalize_newlines(&text[byte_start..byte_end])),
    }
}

/// `\r\n` を `\n` に正規化する（[`snapshot`] が compose 領域を切り出した**後**の
/// `committed` / `compose` 文字列に対して呼ぶ。UTF-16 span は正規化前の元の `text` に
/// 対して解決するため、切り出し前に正規化すると span がずれる）。
fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n")
}

/// `prev.committed` から `next.committed` への文字単位の共通接頭辞をもとに、
/// `(prev 側で消す文字数, next 側で挿入する文字列)` を返す。
///
/// 前提: カーソルは常に確定済みテキストの末尾にある（ソフトキーボードの標準的な逐次
/// 入力を想定）。タッチでカーソルを文中に移動して編集するケースは追跡しない
/// （共通接頭辞より後ろがまるごと入れ替わったとみなし、末尾の Backspace 連打 +
/// 再入力として表現するため、カーソル位置とは無関係に動くが編集そのものは反映される）。
fn committed_diff(prev: &str, next: &str) -> (usize, String) {
    let common = prev
        .chars()
        .zip(next.chars())
        .take_while(|(a, b)| a == b)
        .count();
    let backspaces = prev.chars().count() - common;
    let insert: String = next.chars().skip(common).collect();
    (backspaces, insert)
}

/// [`egui::Event::Key`]（Backspace 1 回分の press + release）を積む。
fn push_backspaces(events: &mut Vec<egui::Event>, count: usize) {
    for _ in 0..count {
        for pressed in [true, false] {
            events.push(egui::Event::Key {
                key: egui::Key::Backspace,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
    }
}

/// [`egui::Event::Key`]（Enter 1 回分の press + release）を積む。
fn push_enter(events: &mut Vec<egui::Event>) {
    for pressed in [true, false] {
        events.push(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
    }
}

/// `text` を `\n` で分割する。
///
/// egui の `TextEdit` は単独の `Event::Text("\n")` を無視するため（改行は
/// `Event::Key { key: Key::Enter, .. }` で表現する契約）、複数行にまたがる挿入・確定
/// 文字列は行ごとに分割し、行の切れ目に `Event::Key::Enter` を挟んで送る必要がある
/// （[`push_text_with_newlines`] / [`push_commit_with_newlines`] 参照）。`\r\n` → `\n` の
/// 正規化は呼び出し元（[`snapshot`]）が `KeyboardSnapshot` 構築時に済ませているため、
/// ここでは `\n` のみを見ればよい。
fn split_lines(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_string).collect()
}

/// 確定済みテキストへの挿入分（`Event::Text` 相当）を、改行境界で
/// `Event::Text` + `Event::Key::Enter` に分解して積む（空セグメントは送らない）。
fn push_text_with_newlines(events: &mut Vec<egui::Event>, text: &str) {
    let mut segments = split_lines(text).into_iter();
    if let Some(first) = segments.next() {
        if !first.is_empty() {
            events.push(egui::Event::Text(first));
        }
    }
    for segment in segments {
        push_enter(events);
        if !segment.is_empty() {
            events.push(egui::Event::Text(segment));
        }
    }
}

/// IME 確定分（`Ime(Commit(..))` 相当）を、改行境界で分解して積む。
///
/// 先頭セグメントのみ `Ime(Commit(..))`（preedit を閉じる意味を持つため空文字列でも
/// 必ず 1 回送る）、2 セグメント目以降は [`push_text_with_newlines`] と同じ規則
/// （`Event::Key::Enter` を挟んでから `Event::Text`、空セグメントは送らない）で送る。
fn push_commit_with_newlines(events: &mut Vec<egui::Event>, text: &str) {
    let mut segments = split_lines(text).into_iter();
    if let Some(first) = segments.next() {
        if first.is_empty() {
            // 空 Commit だけだと egui 側に preedit が残る quirk があるため、先に空
            // Preedit で明示的に閉じてから Commit する（text 全体が空の場合だけでなく、
            // 先頭セグメントが空になる「確定文字列が改行で始まる」ケースも同じ扱いにする）。
            events.push(egui::Event::Ime(egui::ImeEvent::Preedit(String::new())));
        }
        events.push(egui::Event::Ime(egui::ImeEvent::Commit(first)));
    }
    for segment in segments {
        push_enter(events);
        if !segment.is_empty() {
            events.push(egui::Event::Text(segment));
        }
    }
}

/// [`keyboard_events`] の戻り値。`events` は今回発火させる egui イベント列、`prev` は
/// 「これを適用した後、次フレームの `prev` としてそのまま使うべき状態」（呼び出し元は
/// 常にこの値をそのまま次回の `prev` に採用すればよく、defer するかどうかを自前で
/// 判定する必要はない）。
pub(crate) struct AppliedKeyboardEvents {
    pub events: Vec<egui::Event>,
    pub prev: KeyboardSnapshot,
}

/// 前回 [`KeyboardSnapshot`] と今回の差分を egui イベント列へ変換する。
///
/// IME 変換中に Backspace が押された場合（`prev.compose.is_some()` かつ `committed_diff`
/// が backspace を検出したケース）、egui は `ime_enabled` の間 Backspace イベントを
/// 捨てる（`remove_ime_incompatible_events`）ため、まず `Ime(Preedit/Commit(""))` で
/// 未確定領域を閉じるだけに留め、返す `prev`（[`AppliedKeyboardEvents::prev`]）の
/// `compose` を `None` にして返す。実際の `next`（Android 側の実バッファ）はまだ
/// backspace 前の内容のままなので、呼び出し元が次フレームも同じ `next` でこの関数を
/// 再度呼ぶと、今度は `prev.compose.is_none()` の経路に入って実際に Backspace イベントが
/// 発火する（2 フレームがかりで「preedit を閉じる」→「Backspace を送る」の順序を作る）。
pub(crate) fn keyboard_events(
    prev: &KeyboardSnapshot,
    next: &KeyboardSnapshot,
) -> AppliedKeyboardEvents {
    let mut events = Vec::new();
    let (backspaces, insert) = committed_diff(&prev.committed, &next.committed);

    if prev.compose.is_none() {
        push_backspaces(&mut events, backspaces);
        push_text_with_newlines(&mut events, &insert);
        if let Some(compose) = &next.compose {
            events.push(egui::Event::Ime(egui::ImeEvent::Enabled));
            events.push(egui::Event::Ime(egui::ImeEvent::Preedit(compose.clone())));
        }
        return AppliedKeyboardEvents {
            events,
            prev: next.clone(),
        };
    }

    // ここから prev.compose.is_some()（未確定領域が表示中）。
    if backspaces > 0 {
        events.push(egui::Event::Ime(egui::ImeEvent::Preedit(String::new())));
        events.push(egui::Event::Ime(egui::ImeEvent::Commit(String::new())));
        // 今回はまだ Backspace を送っていない（上記 doc 参照）。committed は変えず
        // compose だけ閉じた状態を次回の prev にし、次フレームで同じ next に対し
        // 再評価させて実際の Backspace を送らせる。
        return AppliedKeyboardEvents {
            events,
            prev: KeyboardSnapshot {
                committed: prev.committed.clone(),
                compose: None,
            },
        };
    }

    match &next.compose {
        None => {
            // 空 Commit（先頭セグメントが空）の場合の Preedit("") 先行は
            // push_commit_with_newlines 側に統一済み。
            push_commit_with_newlines(&mut events, &insert);
        }
        Some(compose) => {
            if !insert.is_empty() {
                // 確定した委譲文字列 + 新規コンポーズの開始（例: 変換確定直後に次の文字が打鍵された）。
                push_commit_with_newlines(&mut events, &insert);
                events.push(egui::Event::Ime(egui::ImeEvent::Enabled));
                events.push(egui::Event::Ime(egui::ImeEvent::Preedit(compose.clone())));
            } else if Some(compose) != prev.compose.as_ref() {
                events.push(egui::Event::Ime(egui::ImeEvent::Preedit(compose.clone())));
            }
        }
    }
    AppliedKeyboardEvents {
        events,
        prev: next.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_to_byte_index_ascii() {
        assert_eq!(utf16_to_byte_index("abc", 0), Some(0));
        assert_eq!(utf16_to_byte_index("abc", 2), Some(2));
        // ちょうど末尾（UTF-16 長 == 3）は Some(text.len())、長さを超える位置は None。
        assert_eq!(utf16_to_byte_index("abc", 3), Some(3));
        assert_eq!(utf16_to_byte_index("abc", 10), None);
    }

    #[test]
    fn utf16_to_byte_index_japanese_is_one_unit_three_bytes() {
        // 「あ」は UTF-16 で 1 コード単位、UTF-8 で 3 バイト。
        let text = "あいう";
        assert_eq!(utf16_to_byte_index(text, 0), Some(0));
        assert_eq!(utf16_to_byte_index(text, 1), Some(3));
        assert_eq!(utf16_to_byte_index(text, 2), Some(6));
        assert_eq!(utf16_to_byte_index(text, 3), Some(9));
    }

    #[test]
    fn utf16_to_byte_index_emoji_is_two_units() {
        // 基本多言語面外の絵文字（サロゲートペア）は UTF-16 で 2 コード単位。
        let text = "a😀b";
        assert_eq!(text.len(), 1 + 4 + 1); // 'a'(1) + emoji(4 bytes) + 'b'(1)
        assert_eq!(utf16_to_byte_index(text, 0), Some(0));
        assert_eq!(utf16_to_byte_index(text, 1), Some(1)); // emoji の直前
        assert_eq!(utf16_to_byte_index(text, 3), Some(5)); // emoji(2 units) 分進んだ 'b' の直前
    }

    /// レビュー指摘（codex 6 巡目 P2）: サロゲートペア（絵文字、UTF-16 で 2 コード単位）の
    /// 途中を指す位置は `None`（丸めて別のバイト位置を返さない）。
    #[test]
    fn utf16_to_byte_index_mid_surrogate_pair_is_none() {
        let text = "a😀b";
        assert_eq!(utf16_to_byte_index(text, 2), None);
    }

    #[test]
    fn snapshot_without_compose() {
        let snap = snapshot("hello", None);
        assert_eq!(snap.committed, "hello");
        assert_eq!(snap.compose, None);
    }

    #[test]
    fn snapshot_with_compose_extracts_substring_and_removes_from_committed() {
        // 「かんじ」を変換中、確定済みは前後の "ab" / "cd"。
        let text = "abかんじcd";
        let compose_start_bytes = "ab".len();
        let compose_end_bytes = compose_start_bytes + "かんじ".len();
        // UTF-16 単位に変換（"かんじ" は各 1 unit）。
        let start_units = "ab".chars().count();
        let end_units = start_units + "かんじ".chars().count();
        let _ = (compose_start_bytes, compose_end_bytes);
        let snap = snapshot(text, Some((start_units, end_units)));
        assert_eq!(snap.committed, "abcd");
        assert_eq!(snap.compose.as_deref(), Some("かんじ"));
    }

    #[test]
    fn snapshot_with_broken_span_falls_back_to_no_compose() {
        let snap = snapshot("hello", Some((100, 200)));
        assert_eq!(snap.committed, "hello");
        assert_eq!(snap.compose, None);
    }

    #[test]
    fn snapshot_with_zero_width_span_falls_back_to_no_compose() {
        let snap = snapshot("hello", Some((2, 2)));
        assert_eq!(snap.committed, "hello");
        assert_eq!(snap.compose, None);
    }

    /// レビュー指摘（codex 6 巡目 P2）: 片端だけが文字列長を超える span は、
    /// 丸めた位置（旧実装では `Some("ello")`）ではなく未確定領域なしとして扱う。
    #[test]
    fn snapshot_with_end_beyond_text_length_falls_back_to_no_compose() {
        let snap = snapshot("hello", Some((1, 200)));
        assert_eq!(snap.committed, "hello");
        assert_eq!(snap.compose, None);
    }

    /// レビュー指摘（codex 6 巡目 P2）: span の端がサロゲートペアの途中を指す場合も
    /// 未確定領域なしとして扱う。
    #[test]
    fn snapshot_with_mid_surrogate_span_falls_back_to_no_compose() {
        let text = "a😀b";
        let snap = snapshot(text, Some((1, 2)));
        assert_eq!(snap.committed, text);
        assert_eq!(snap.compose, None);
    }

    fn key_presses(events: &[egui::Event]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, egui::Event::Key { pressed: true, .. }))
            .count()
    }

    #[test]
    fn ascii_sequential_input_produces_text_events() {
        let prev = KeyboardSnapshot {
            committed: "ab".to_string(),
            compose: None,
        };
        let next = KeyboardSnapshot {
            committed: "abc".to_string(),
            compose: None,
        };
        let applied = keyboard_events(&prev, &next);
        assert_eq!(applied.events, vec![egui::Event::Text("c".to_string())]);
        assert_eq!(applied.prev, next);
    }

    #[test]
    fn japanese_compose_then_commit() {
        // 1. 未確定なし → 「か」を変換中（compose 開始）。
        let prev = KeyboardSnapshot::default();
        let composing = KeyboardSnapshot {
            committed: String::new(),
            compose: Some("か".to_string()),
        };
        let applied = keyboard_events(&prev, &composing);
        assert_eq!(
            applied.events,
            vec![
                egui::Event::Ime(egui::ImeEvent::Enabled),
                egui::Event::Ime(egui::ImeEvent::Preedit("か".to_string())),
            ]
        );
        assert_eq!(applied.prev, composing);

        // 2. 変換候補が「漢字」に変わる（compose 更新）。
        let composing2 = KeyboardSnapshot {
            committed: String::new(),
            compose: Some("漢字".to_string()),
        };
        let applied = keyboard_events(&composing, &composing2);
        assert_eq!(
            applied.events,
            vec![egui::Event::Ime(egui::ImeEvent::Preedit(
                "漢字".to_string()
            ))]
        );
        assert_eq!(applied.prev, composing2);

        // 3. 確定（compose が消え、committed に入る）。
        let committed = KeyboardSnapshot {
            committed: "漢字".to_string(),
            compose: None,
        };
        let applied = keyboard_events(&composing2, &committed);
        assert_eq!(
            applied.events,
            vec![egui::Event::Ime(egui::ImeEvent::Commit("漢字".to_string()))]
        );
        assert_eq!(applied.prev, committed);
    }

    #[test]
    fn backspace_while_composing_closes_preedit_first_and_keeps_compose_pending() {
        let composing = KeyboardSnapshot {
            committed: "x".to_string(),
            compose: Some("か".to_string()),
        };
        // 確定済み部分が 1 文字減る（Backspace 相当）。
        let next = KeyboardSnapshot {
            committed: String::new(),
            compose: Some("か".to_string()),
        };
        let applied = keyboard_events(&composing, &next);
        assert_eq!(
            applied.events,
            vec![
                egui::Event::Ime(egui::ImeEvent::Preedit(String::new())),
                egui::Event::Ime(egui::ImeEvent::Commit(String::new())),
            ]
        );
        // 次フレームで実際に Backspace を送らせるため、committed は変えず compose だけ閉じる。
        assert_eq!(
            applied.prev,
            KeyboardSnapshot {
                committed: "x".to_string(),
                compose: None,
            }
        );
    }

    /// レビュー指摘（must）: `defer`（preedit を閉じるだけの 1 フレーム目）の後、
    /// 呼び出し元が `applied.prev` をそのまま次回の `prev` に採用すれば、
    /// 2 フレーム目で同じ `next` を渡すだけで実際に Backspace が発火することを確認する
    /// （修正前は呼び出し側が `prev` を更新しないまま同じ入力で永久に defer し続けるバグがあった）。
    #[test]
    fn backspace_while_composing_fires_backspace_on_second_frame() {
        let composing = KeyboardSnapshot {
            committed: "x".to_string(),
            compose: Some("か".to_string()),
        };
        let next = KeyboardSnapshot {
            committed: String::new(),
            compose: Some("か".to_string()),
        };

        // 1 フレーム目: preedit を閉じるだけで Backspace はまだ出ない。
        let first = keyboard_events(&composing, &next);
        assert_eq!(
            first.events,
            vec![
                egui::Event::Ime(egui::ImeEvent::Preedit(String::new())),
                egui::Event::Ime(egui::ImeEvent::Commit(String::new())),
            ]
        );

        // 2 フレーム目: 呼び出し元は `first.prev` をそのまま `prev` として使い、
        // 同じ `next`（Android 側バッファは変化していない）で再評価する。
        let second = keyboard_events(&first.prev, &next);
        assert_eq!(key_presses(&second.events), 1);
        assert!(second
            .events
            .contains(&egui::Event::Ime(egui::ImeEvent::Enabled)));
        assert!(second
            .events
            .contains(&egui::Event::Ime(egui::ImeEvent::Preedit("か".to_string()))));
        assert_eq!(second.prev, next);
    }

    #[test]
    fn backspace_after_commit_produces_backspace_key_events() {
        let prev = KeyboardSnapshot {
            committed: "abc".to_string(),
            compose: None,
        };
        let next = KeyboardSnapshot {
            committed: "ab".to_string(),
            compose: None,
        };
        let applied = keyboard_events(&prev, &next);
        assert_eq!(key_presses(&applied.events), 1);
        assert_eq!(applied.events.len(), 2); // press + release
    }

    #[test]
    fn no_diff_produces_empty_events() {
        let snap = KeyboardSnapshot {
            committed: "same".to_string(),
            compose: Some("x".to_string()),
        };
        let applied = keyboard_events(&snap, &snap);
        assert!(applied.events.is_empty());
        assert_eq!(applied.prev, snap);
    }

    /// レビュー指摘（codex 3 件目）: egui `TextEdit` は単独の `Event::Text("\n")` を
    /// 無視するため、改行を含む挿入文字列は行ごとに `Event::Text` + `Event::Key::Enter`
    /// へ分解しなければならない。compose なし（`prev.compose.is_none()`）の経路を確認する。
    #[test]
    fn insert_with_newline_splits_into_text_and_enter_key_events() {
        let prev = KeyboardSnapshot::default();
        let next = KeyboardSnapshot {
            committed: "a\nb".to_string(),
            compose: None,
        };
        let applied = keyboard_events(&prev, &next);
        assert_eq!(
            applied.events,
            vec![
                egui::Event::Text("a".to_string()),
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Text("b".to_string()),
            ]
        );
        assert_eq!(applied.prev, next);
    }

    /// preedit 表示中に改行を含む確定文字列が来た場合、先頭セグメントは
    /// `Ime(Commit(..))` で preedit を閉じ、残りは `Event::Text` + `Event::Key::Enter` で送る。
    #[test]
    fn commit_with_newline_while_composing_closes_preedit_then_sends_enter() {
        let prev = KeyboardSnapshot {
            committed: String::new(),
            compose: Some("か".to_string()),
        };
        let next = KeyboardSnapshot {
            committed: "a\nb".to_string(),
            compose: None,
        };
        let applied = keyboard_events(&prev, &next);
        assert_eq!(
            applied.events,
            vec![
                egui::Event::Ime(egui::ImeEvent::Commit("a".to_string())),
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Text("b".to_string()),
            ]
        );
        assert_eq!(applied.prev, next);
    }

    /// レビュー指摘（codex 4 件目）: 確定文字列が改行で始まる場合、先頭セグメントが
    /// 空文字列になる（`push_commit_with_newlines` は `Commit("")` を送る）。
    /// 空 Commit だけだと egui 側に旧 preedit が残る quirk があるため、
    /// 先に `Ime(Preedit(""))` を送ってから `Ime(Commit(""))` を送ることを確認する。
    #[test]
    fn commit_starting_with_newline_closes_preedit_before_empty_commit() {
        let prev = KeyboardSnapshot {
            committed: String::new(),
            compose: Some("か".to_string()),
        };
        let next = KeyboardSnapshot {
            committed: "\nb".to_string(),
            compose: None,
        };
        let applied = keyboard_events(&prev, &next);
        assert_eq!(
            applied.events,
            vec![
                egui::Event::Ime(egui::ImeEvent::Preedit(String::new())),
                egui::Event::Ime(egui::ImeEvent::Commit(String::new())),
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Text("b".to_string()),
            ]
        );
        assert_eq!(applied.prev, next);
    }

    /// レビュー指摘（codex 5 巡目）: 改行の正規化を `split_lines`（出力時）だけでなく
    /// `snapshot`（`committed`/`compose` の構築時）でも行うことを確認する。正規化前は
    /// `committed_diff` が `"a\r\n"` を 2 文字（`\r` と `\n`）の差分として数え、
    /// Backspace が 2 回発火する不具合があった（CRLF は 1 文字の改行として 1 回で足りる）。
    #[test]
    fn crlf_to_no_newline_transition_fires_single_backspace() {
        let prev = snapshot("a\r\n", None);
        let next = snapshot("a", None);
        let applied = keyboard_events(&prev, &next);
        assert_eq!(
            applied.events,
            vec![
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        );
        assert_eq!(applied.prev, next);
    }
}
