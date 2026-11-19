//! `.icon()` / `.icon_bound()` の記号の描画と AX を検証する（Issue #1437）。
//!
//! 記号は `IconName::glyph` の Unicode 1 文字を、CJK を入れない記号専用の family（emoji-icon-font・
//! Noto Emoji）で描く。字形が無いと豆腐になり、CJK が先頭だと一部の字を取られるため、全名前が専用
//! family に字形を持ち CJK 読み込み後も専用 family の字形で描かれることを固定する。
//! 文字は painter で直接描くので AX ノードは増えず、読み上げは合成の Image ノード 1 つ（名前はラベル。空なら装飾用でノードなし）だけになる。

use fandhe_core::{IconName, Screen, StateStore, WidgetId};

struct St;

fn id(n: u64) -> WidgetId {
    WidgetId::from_raw(n)
}

fn bound_name(_: &St) -> IconName {
    IconName::Heart
}

fn shape_text(shape: &egui::Shape, out: &mut String) {
    match shape {
        egui::Shape::Vec(v) => v.iter().for_each(|s| shape_text(s, out)),
        egui::Shape::Text(t) => {
            out.push_str(t.galley.text());
            out.push('|');
        }
        _ => {}
    }
}

/// 数フレーム描画して、出た文字列（`|` 区切り）を返す。
fn painted_text(screen: &Screen<St>) -> String {
    let ctx = egui::Context::default();
    let mut store = StateStore::new(St);
    let mut text = String::new();
    for _ in 0..3 {
        let full = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    fandhe_render_egui::render_screen(ui, screen, &mut store);
                });
            },
        );
        text.clear();
        for c in &full.shapes {
            shape_text(&c.shape, &mut text);
        }
    }
    text
}

/// 固定名・状態束縛の名前が、それぞれの記号として描かれる。名前ごとに別の文字になる。
#[test]
fn icons_are_painted_as_their_glyph() {
    let text = painted_text(
        &Screen::new("t")
            .icon(id(1), IconName::Settings, "設定")
            .icon_bound(id(2), bound_name, "お気に入り"),
    );
    for name in [IconName::Settings, IconName::Heart] {
        assert!(text.contains(name.glyph()), "{name:?}: {text}");
    }
    assert!(!text.contains(IconName::Home.glyph()), "{text}");
    // ラベルは AX の名前であって、画面に文字としては出さない。
    assert!(
        !text.contains("設定") && !text.contains("お気に入り"),
        "{text}"
    );
}

fn icon_family() -> egui::FontFamily {
    egui::FontFamily::Name("fandhe-icon".into())
}

/// CJK を読み込んだ context。記号専用の family が登録される。
fn ctx_with_cjk() -> egui::Context {
    let ctx = egui::Context::default();
    fandhe_render_egui::font::apply_font_bytes(&ctx, fandhe_fonts::NOTO_SANS_JP_REGULAR.to_vec());
    let _ = ctx.run(Default::default(), |_| {});
    ctx
}

/// 語彙の全名前が、記号専用の family の中に字形を持つ（豆腐にならない）。
#[test]
fn every_glyph_exists_in_the_icon_family() {
    let ctx = ctx_with_cjk();
    let font = egui::FontId::new(24.0, icon_family());
    for name in IconName::ALL {
        assert!(
            ctx.fonts_mut(|f| f.has_glyph(&font, name.glyph())),
            "{name:?}（{:?}）が記号専用 family に無い",
            name.glyph()
        );
    }
}

/// CJK を読み込んだ後も、CJK が字形を取ってしまう 6 字（⬇ ⬆ ⚠ ★ ♥ ▶）は専用 family の字形で描く。
/// `Proportional` で引くと Noto Sans JP の字形（幅が違う）になる。
#[test]
fn cjk_does_not_take_icon_glyphs() {
    let ctx = ctx_with_cjk();
    let icon = egui::FontId::new(24.0, icon_family());
    let prop = egui::FontId::proportional(24.0);
    for name in [
        IconName::Download,
        IconName::Upload,
        IconName::Warning,
        IconName::Star,
        IconName::Heart,
        IconName::Play,
    ] {
        let c = name.glyph();
        // 字形の大きさ（ラスタライズした画像の寸法とオフセット）は、フォントが違えば変わる。
        let shape = |font: &egui::FontId| {
            let galley = ctx
                .fonts_mut(|f| f.layout_no_wrap(c.to_string(), font.clone(), egui::Color32::WHITE));
            let uv = galley.rows[0].glyphs[0].uv_rect;
            (uv.size, uv.offset)
        };
        assert_ne!(
            shape(&icon),
            shape(&prop),
            "{name:?}: 専用 family が CJK の字形と同じ"
        );
    }
}

fn text_families(shape: &egui::Shape, out: &mut Vec<egui::FontFamily>) {
    match shape {
        egui::Shape::Vec(v) => v.iter().for_each(|s| text_families(s, out)),
        egui::Shape::Text(t) => out.extend(
            t.galley
                .job
                .sections
                .iter()
                .map(|sec| sec.format.font_id.family.clone()),
        ),
        _ => {}
    }
}

/// 描画は CJK を読み込んだ context で、記号を専用 family で描く（`Proportional` では CJK が字を取る）。
#[test]
fn icon_is_drawn_with_the_icon_family() {
    let ctx = ctx_with_cjk();
    let screen = Screen::new("t").icon(id(1), IconName::Star, "星");
    let mut store = StateStore::new(St);
    let mut families = Vec::new();
    for _ in 0..2 {
        let full = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                fandhe_render_egui::render_screen(ui, &screen, &mut store);
            });
        });
        families.clear();
        full.shapes
            .iter()
            .for_each(|c| text_families(&c.shape, &mut families));
    }
    assert!(families.contains(&icon_family()), "{families:?}");
}

/// CJK なし（Web でフォントの取得に失敗した経路）でも専用 family を登録し、全名前の字形がそこから出る。
#[test]
fn icon_family_is_registered_without_cjk() {
    let ctx = egui::Context::default();
    fandhe_render_egui::font::apply_fonts(&ctx, None);
    let _ = ctx.run(Default::default(), |_| {});
    let font = egui::FontId::new(24.0, icon_family());
    for name in IconName::ALL {
        assert!(
            ctx.fonts_mut(|f| f.has_glyph(&font, name.glyph())),
            "{name:?}"
        );
    }
}

/// 専用 family が未登録の context（CJK を読み込んでいない既定フォント）でも panic せず、記号を描く。
#[test]
fn falls_back_to_proportional_without_icon_family() {
    let text = painted_text(&Screen::new("t").icon(id(1), IconName::Star, "星"));
    assert!(text.contains(IconName::Star.glyph()), "{text}");
}

#[cfg(any(target_os = "macos", target_os = "android", target_os = "windows"))]
mod ax {
    use super::*;
    use egui_kittest::kittest::Queryable;

    fn harness(screen: Screen<St>) -> egui_kittest::Harness<'static, StateStore<St>> {
        let mut h = egui_kittest::Harness::builder()
            .with_size(egui::vec2(400.0, 300.0))
            .build_ui_state(
                move |ui, store: &mut StateStore<St>| {
                    fandhe_render_egui::render_screen(ui, &screen, store);
                },
                StateStore::new(St),
            );
        h.run();
        h
    }

    fn images(h: &egui_kittest::Harness<'_, StateStore<St>>) -> usize {
        h.query_all_by_role(egui::accesskit::Role::Image).count()
    }

    /// ラベルは Image の名前として 1 ノードだけに出る（記号の文字は AX に出ない）。
    #[test]
    fn label_is_the_name_of_one_image_node() {
        let h = harness(Screen::new("t").icon(id(1), IconName::Settings, "設定を開く"));
        assert_eq!(h.query_all_by_label("設定を開く").count(), 1);
        assert_eq!(images(&h), 1);
        assert_eq!(
            h.query_all_by_label(&IconName::Settings.glyph().to_string())
                .count(),
            0
        );
    }

    /// 状態束縛でもラベルが名前になる。
    #[test]
    fn bound_label_is_the_name() {
        let h = harness(Screen::new("t").icon_bound(id(1), bound_name, "お気に入り"));
        assert_eq!(h.query_all_by_label("お気に入り").count(), 1);
    }

    /// ラベルが空のアイコンは装飾用で、読み上げのノードを作らない。
    #[test]
    fn empty_label_is_decorative() {
        let h = harness(Screen::new("t").icon(id(1), IconName::Settings, ""));
        assert_eq!(images(&h), 0);
    }
}
