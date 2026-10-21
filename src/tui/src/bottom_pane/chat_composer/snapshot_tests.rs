//! Snapshot coverage for draft text, shortcuts, and composer rendering.

use super::tests::new_test_composer;
use crate::render::renderable::Renderable;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

// 008-TRIM: fn plugin_mention_uses_display_name_for_opaque_id（覆盖被裁 plugins/plugin
// mention（set_plugin_mentions、codex_plugin::PluginCapabilitySummary、test_plugin_summary
// 辅助），008 §1）

// 008-TRIM: fn shortcut_footer_displays_configured_chords（覆盖被裁 shortcut overlay 渲染（008 §1，footer.rs 已裁剪为空渲染））

#[test]
fn draft_and_voice_composer_snapshots() {
    // 008-ADAPT: 母本循环对比 voice_active ∈ {false, true} 两分支；voice strip 属被裁
    // 功能（008 §1，set_voice_strip/voice_strip 模块未移植），仅保留 draft 分支，
    // 快照名 "draft_composer" 与断言不变。
    crate::terminal_palette::with_test_default_colors(
        crate::terminal_probe::DefaultColors {
            fg: (230, 216, 255),
            bg: (36, 27, 53),
        },
        || {
            let (mut composer, _rx) = new_test_composer();
            composer.set_text_content(
                "Explore the night sky".to_string(),
                Vec::new(),
                Vec::new(),
            );
            let width = 60;
            let area = Rect::new(
                /*x*/ 0,
                /*y*/ 0,
                width,
                composer.desired_height(width),
            );
            let mut buffer = Buffer::empty(area);
            composer.render(area, &mut buffer);
            let rows = buffer
                .content
                .chunks(usize::from(width))
                .map(|row| {
                    row.iter()
                        .map(ratatui::buffer::Cell::symbol)
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");
            insta::assert_snapshot!("draft_composer", rows);
        },
    )
}
