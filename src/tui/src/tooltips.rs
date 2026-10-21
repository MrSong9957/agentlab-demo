use crate::keymap::RuntimeKeymap;
use crate::keymap::keymap_action_id;
use crate::terminal_hyperlinks::HyperlinkLine;
use codex_config::types::TuiKeymap;
use codex_features::FEATURES;
use codex_features::Feature;
use codex_features::FeatureSpec;
use codex_protocol::account::PlanType;
use lazy_static::lazy_static;
use rand::Rng;
use rand::seq::IteratorRandom;
use std::path::Path;

#[cfg(test)]
#[path = "tooltips/keybinding_tests.rs"]
mod keybinding_tests;

// 008-TRIM: ANNOUNCEMENT_TIP_URL（远端公告，008 §1）

const IS_MACOS: bool = cfg!(target_os = "macos");
const IS_WINDOWS: bool = cfg!(target_os = "windows");

const WINDOWS_APP_TOOLTIP: &str = "Use the **desktop app**. Install it from https://chatgpt.com/codex?app-landing-page=true and run `codex app`.";
const MACOS_APP_TOOLTIP: &str =
    "Use the **desktop app**. Run `codex app` to open it. It installs automatically if needed.";
const LINUX_APP_TOOLTIP: &str = "Use the **desktop app**. Install it from https://learn.chatgpt.com/docs/linux/linux-app and run `chatgpt`.";

const RAW_TOOLTIPS: &str = include_str!("../assets/tooltips.txt");

lazy_static! {
    static ref TOOLTIPS: Vec<&'static str> = RAW_TOOLTIPS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .chain(app_tooltip())
        .collect();
    // 008-TRIM: experimental_tooltips/ALL_TOOLTIPS（实验功能公告与 realtime 语音无本地来源，008 §1）
    static ref ALL_TOOLTIPS: Vec<&'static str> = TOOLTIPS.clone();
}

/// Pick a random tooltip to show to the user when starting Codex.
pub(crate) fn get_tooltip(plan: Option<PlanType>, keymap: &TuiKeymap) -> Option<String> {
    let mut rng = rand::rng();
    // 008-TRIM: preferred_tooltip（远端公告提示需 codex_http_client，无本地来源；本地仅本地 tips，008 §1）
    let _ = plan;
    pick_tooltip(&mut rng, keymap)
}

struct LinuxDesktopSession {
    has_display: bool,
    is_wsl: bool,
}

impl LinuxDesktopSession {
    fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self {
                has_display: std::env::var_os("DISPLAY").is_some_and(|value| !value.is_empty())
                    || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty()),
                is_wsl: crate::clipboard_paste::is_probably_wsl(),
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            Self {
                has_display: false,
                is_wsl: false,
            }
        }
    }
}

fn linux_app_tooltip(session: LinuxDesktopSession) -> Option<&'static str> {
    (session.has_display && !session.is_wsl).then_some(LINUX_APP_TOOLTIP)
}

fn app_tooltip() -> Option<&'static str> {
    if IS_MACOS {
        Some(MACOS_APP_TOOLTIP)
    } else if IS_WINDOWS {
        Some(WINDOWS_APP_TOOLTIP)
    } else {
        linux_app_tooltip(LinuxDesktopSession::current())
    }
}

fn pick_tooltip<R: Rng + ?Sized>(rng: &mut R, keymap: &TuiKeymap) -> Option<String> {
    // Resolve current settings for each new tip; never replace an invalid or unbound keymap
    // with defaults, or cache shortcut text across /keymap edits.
    let keymap = RuntimeKeymap::from_config(keymap).ok();
    resolved_tooltips(keymap.as_ref()).choose(rng)
}

/// Render shared tip styling and links, retaining visible URLs when the terminal needs them.
pub(crate) fn render_tooltip_lines(tip: &str, width: usize, cwd: &Path) -> Vec<HyperlinkLine> {
    crate::markdown_render::render_streaming_markdown_lines_with_width_and_cwd(
        &format!("**Tip:** {tip}"),
        Some(width),
        Some(cwd),
        &crate::markdown_render::hide_web_link_destination,
        crate::markdown_render::ListSpacing::AfterMultiline,
    )
    .lines
}

/// Resolve the local tip pool in catalog order using the supplied runtime keymap.
/// Tips with invalid or unbound shortcuts are omitted; without a keymap, only key-free tips remain.
pub(crate) fn resolved_tooltips(
    keymap: Option<&RuntimeKeymap>,
) -> impl Iterator<Item = String> + '_ {
    ALL_TOOLTIPS
        .iter()
        .filter_map(move |tip| render_tooltip(tip, keymap))
}

pub(crate) fn tooltip_templates() -> impl Iterator<Item = &'static str> {
    ALL_TOOLTIPS.iter().copied()
}

/// Substitute `{key:context.action}` with the current primary shortcut in a Markdown code span.
/// Skip the tip if a placeholder is invalid or its action has no binding.
pub(crate) fn render_tooltip(mut template: &str, keymap: Option<&RuntimeKeymap>) -> Option<String> {
    let mut rendered = String::new();
    while let Some((prefix, rest)) = template.split_once("{key:") {
        let (action, suffix) = rest.split_once('}')?;
        let (context, action) = action.split_once('.')?;
        let action = keymap_action_id(context, action)?;
        let hint = keymap?.primary_hint(action.context, action.action)?;
        rendered.push_str(prefix);
        // A key or two-key chord can contain literal backticks; use a padded code span.
        rendered.push_str(&format!("`` {} ``", hint.display_label()));
        template = suffix;
    }
    rendered.push_str(template);
    Some(rendered)
}

// 008-TRIM: mod announcement（远端公告获取：codex_http_client + 账户计划定向，无本地来源，008 §1）


#[cfg(test)]
mod tests {
    use super::*;
    // 008-TRIM: use crate::tooltips::announcement::AnnouncementTips（远端公告测试裁剪残留，008 §1）
    use pretty_assertions::assert_eq;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    // 008-TRIM: fn experimental_voice_tooltip_requires_runtime_support（对应被裁远端/实验功能，008 §1）

    #[test]
    fn random_tooltip_returns_some_tip_when_available() {
        let mut rng = StdRng::seed_from_u64(42);
        assert!(pick_tooltip(&mut rng, &TuiKeymap::default()).is_some());
    }

    #[test]
    fn random_tooltip_is_reproducible_with_seed() {
        let expected = {
            let mut rng = StdRng::seed_from_u64(7);
            pick_tooltip(&mut rng, &TuiKeymap::default())
        };

        let mut rng = StdRng::seed_from_u64(7);
        assert_eq!(expected, pick_tooltip(&mut rng, &TuiKeymap::default()));
    }

    #[test]
    fn desktop_app_tips_render_at_narrow_width() {
        let cwd = std::env::current_dir().unwrap();
        for (platform, tip) in [
            ("macos", MACOS_APP_TOOLTIP),
            ("windows", WINDOWS_APP_TOOLTIP),
            ("linux", LINUX_APP_TOOLTIP),
        ] {
            let rendered = render_tooltip_lines(tip, /*width*/ 40, &cwd)
                .iter()
                .map(|line| format!("{:?} {:?}", line.line, line.hyperlinks))
                .collect::<Vec<_>>()
                .join("\n");
            insta::assert_snapshot!(format!("desktop_app_tip_{platform}"), rendered);
        }
    }

    #[test]
    fn desktop_app_tooltip_uses_supported_platform_launcher() {
        let tooltip = TOOLTIPS
            .iter()
            .copied()
            .find(|tip| tip.contains("desktop app"));

        if linux_app_tooltip(LinuxDesktopSession::current()).is_some() {
            let tooltip = tooltip.expect("Linux should advertise the desktop app");
            assert_eq!(app_tooltip(), Some(tooltip));
        } else if IS_MACOS {
            let tooltip = tooltip.expect("macOS should advertise the desktop app");
            assert_eq!(app_tooltip(), Some(tooltip));
        } else if IS_WINDOWS {
            let tooltip = tooltip.expect("Windows should advertise the desktop app");
            assert_eq!(app_tooltip(), Some(tooltip));
        } else {
            assert_eq!(tooltip, None);
            assert_eq!(app_tooltip(), None);
        }
    }

    #[test]
    fn linux_desktop_app_tooltip_requires_graphical_native_session() {
        assert_eq!(
            linux_app_tooltip(LinuxDesktopSession {
                has_display: true,
                is_wsl: false,
            }),
            Some(LINUX_APP_TOOLTIP)
        );

        assert_eq!(
            linux_app_tooltip(LinuxDesktopSession {
                has_display: false,
                is_wsl: false,
            }),
            None
        );
        assert_eq!(
            linux_app_tooltip(LinuxDesktopSession {
                has_display: true,
                is_wsl: true,
            }),
            None
        );
    }

    // 008-TRIM: fn plans_use_platform_app_tip（对应被裁远端/实验功能，008 §1）

    // 008-TRIM: fn announcement_tip_toml_picks_last_matching（对应被裁远端/实验功能，008 §1）
    // 008-TRIM: fn announcement_tip_toml_rejects_unknown_target_os（对应被裁远端/实验功能，008 §1）
}
