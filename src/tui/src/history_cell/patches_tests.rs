use super::*;
use pretty_assertions::assert_eq;

#[test]
fn compact_patches_retain_full_changes_and_failure_details() {
    let patch = new_patch_event(
        HashMap::from([
            (
                PathBuf::from("new.txt"),
                FileChange::Add {
                    content: "one\ntwo\nthree\nfour\nfive\n".into(),
                },
            ),
            (
                PathBuf::from("existing.txt"),
                FileChange::Update {
                    unified_diff: "@@ -1,3 +1,3 @@\n context\n-before\n+after\n tail\n".into(),
                    move_path: None,
                },
            ),
        ]),
        Path::new("."),
    );
    let failure = new_patch_apply_failure("first\nsecond\nthird\nfourth diagnostic".into());
    let missing = new_patch_apply_failure(String::new());
    let mut snapshots = Vec::new();
    for (label, cell) in [
        ("patch", &patch as &dyn HistoryCell),
        ("failure", &failure),
        ("missing diagnostics", &missing),
    ] {
        let compact = visible_lines(cell.compact_hyperlink_lines(/*width*/ 40));
        let full = visible_lines(cell.transcript_hyperlink_lines(/*width*/ 40));
        snapshots.push(format!(
            "{label}\nCompact\n{}\nFull\n{}",
            Text::from(compact),
            Text::from(full)
        ));
    }
    insta::assert_snapshot!(snapshots.join("\n\n"));
}

/// 019 恢复（上游 008 裁剪用例适配）：查看图片卡片——摘要显示文件名（窄屏截断），
/// 详情/转录保留完整路径，附文本尺寸说明。
#[test]
fn viewed_image_summary_and_details() {
    let cell = new_view_image_event(
        "截图 sample.png".into(),
        "/tmp/dir/截图 sample.png".into(),
        Some((2048, 768)),
    );
    // 完整宽度：文件名＋尺寸
    let full = Text::from(cell.display_lines(/*width*/ 60)).to_string();
    assert!(full.contains("Viewed image"), "标题：{full}");
    assert!(full.contains("截图 sample.png"), "文件名：{full}");
    assert!(full.contains("2048×768"), "尺寸说明：{full}");
    // 转录/详情保留完整路径
    let transcript = Text::from(cell.transcript_lines(/*width*/ 100)).to_string();
    assert!(transcript.contains("/tmp/dir/截图 sample.png"), "路径：{transcript}");
    // 窄屏：截断为省略号，不溢出
    let narrow = cell.display_lines(/*width*/ 10);
    for line in narrow {
        assert!(line.width() <= 10, "窄屏不溢出：{line:?}");
    }
    // 活动ID（disclosure 关联）
    assert_eq!(cell.activity_ids().len(), 1);
    // 控制字符不进入渲染（路径与文件名在宿主侧已过滤；此处防御）
    let hostile = new_view_image_event("a\nb.png".into(), "x\ty".into(), None);
    let text = Text::from(hostile.display_lines(80)).to_string();
    assert_eq!(text.matches('\n').count(), 0, "无换行注入：{text:?}");
}

// 008-TRIM: fn viewed_image_retains_original_path_in_details（覆盖被裁 ImageView（new_view_image_tool_call cell），008 §1）

// 008-TRIM: fn viewed_image_narrow_summary（覆盖被裁 ImageView（new_view_image_tool_call cell），008 §1）

#[test]
fn failed_patch_keeps_diagnostics_beyond_the_legacy_preview() {
    let diagnostics = (1..=12)
        .map(|line| format!("diagnostic line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let cell = new_patch_apply_failure(diagnostics.clone());
    assert_eq!(
        cell.raw_lines(),
        std::iter::once(Line::from("Failed to apply patch"))
            .chain(diagnostics.lines().map(|line| Line::from(line.to_owned())))
            .collect::<Vec<_>>()
    );
}
