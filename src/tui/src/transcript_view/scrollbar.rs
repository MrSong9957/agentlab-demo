//! 011：历史区右侧滚动条（本地新增——固定母本无滚动条组件；ratatui facade 已
//! re-export `Scrollbar`，零新增依赖）。
//!
//! 设计（规格 `docs/specs/011-tui-transcript-scrollbar.md`）：
//! - **不另建滚动模型**——总行数与视口顶行都从 `TranscriptView` 的既有布局/锚
//!   （`start()`/`bottom_start()`/`LayoutCache`）推导；滚动一律经既有 `scroll()`
//!   的钳制与跟随语义（拖到底＝恢复跟随、到顶钳制）。
//! - **沟位常驻**（K1）：历史区最右 1 列恒预留，内容按剩余宽度折行；内容不足
//!   一屏时沟位留空（A1），不出现滑块。
//! - **命中判定与渲染同一公式**：拇指区间复刻 ratatui `Scrollbar::part_lengths`
//!   的整数舍入（无箭头符号），保证点击/拖动的命中行与屏幕上画的滑块一致。

use std::collections::HashMap;
use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Scrollbar;
use ratatui::widgets::ScrollbarOrientation;
use ratatui::widgets::ScrollbarState;
use ratatui::widgets::StatefulWidget as _;

use super::EntryKey;
use super::HistoryCell;
use super::TranscriptView;

/// 行数持久缓存（坑 2：布局缓存只有 64 条近期条目，全量 Σ 会把可见窗口挤出去
/// 且每帧重折行）。以 cell 指针身份（Weak 校验防地址复用）＋宽度为键；宽度
/// 变化或呈现态变化（Ctrl+T／聚焦态展开折叠）由调用方 [`RowCache::clear`]
/// 显式失效。live 尾部（`EntryKey::Live`）不缓存，逐帧现算。
#[derive(Default)]
pub(crate) struct RowCache {
    width: u16,
    entries: HashMap<usize, (std::sync::Weak<dyn HistoryCell>, usize)>,
}

impl RowCache {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// 第 `index` 个条目的行数：命中缓存 O(1)；未命中走 `layout()` 计量并缓存
    /// （新 cell 追加频率＝流式速度，代价有界）。地址复用经 Weak 校验排除。
    fn rows(
        &mut self,
        view: &mut TranscriptView,
        cells: &[Arc<dyn HistoryCell>],
        index: usize,
    ) -> usize {
        let width = view.area.width;
        if self.width != width {
            self.entries.clear();
            self.width = width;
        }
        let snapshot = view.snapshot_cells();
        let cells = snapshot.as_deref().unwrap_or(cells);
        let Some(cell) = cells.get(index) else {
            return 0;
        };
        let key = Arc::as_ptr(cell) as *const () as usize;
        let fresh = |view: &mut TranscriptView| {
            view.layout(cells, index).map_or(/*default*/ 0, |l| l.row_count())
        };
        match self.entries.get(&key) {
            Some((weak, count)) if weak.upgrade().is_some_and(|live| Arc::ptr_eq(&live, cell)) => {
                *count
            }
            _ => {
                // 防地址复用之外的死键堆积：粗粒度收缩（罕见全量重算）
                if self.entries.len() > cells.len().saturating_mul(4).max(64) {
                    self.entries.clear();
                }
                let count = fresh(view);
                self.entries.insert(key, (Arc::downgrade(cell), count));
                count
            }
        }
    }
}

/// 滚动条几何：内容总行数、视口顶行绝对行号与视口行数（均与渲染同帧推导）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScrollbarGeometry {
    pub(crate) total_rows: usize,
    pub(crate) top_row: usize,
    pub(crate) viewport_rows: usize,
}

impl ScrollbarGeometry {
    /// 最大可滚动顶行（= 总行数−视口行数）。ratatui 的 position 语义是
    /// 「0..=content_length−1 的可滚动位置」，因此组件须传 `content_length =
    /// max_top_row() + 1` 而非正文总行数——直接传总行数会把 position 范围
    /// 撑大 (viewport−1) 行，滑块比例偏短、贴底时够不到轨道底、拖动映射
    /// 出现底部死区（ratatui-widgets 0.3.2 scrollbar.rs `part_lengths`：
    /// thumb = round(viewport×track / (max_position+viewport))）。
    fn max_top_row(&self) -> usize {
        self.total_rows.saturating_sub(self.viewport_rows)
    }
}

/// 拖动状态（按下归属）：记录抓取点在滑块内的偏移（沟位行），拖动时以指针位置
/// 反推滑块应处的轨道区间，再按渲染公式的逆映射换算内容顶行——指针到轨道顶
/// 即内容顶行 0、到轨道底即最底（恢复跟随）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScrollDrag {
    grab_offset: u16,
}

/// ratatui `Scrollbar::part_lengths` 的舍入除法（四舍五入向上）。
const fn rounding_divide(numerator: usize, denominator: usize) -> usize {
    (numerator + denominator / 2) / denominator
}

/// 拇指区间（无箭头符号，轨道长＝沟位高）：与 ratatui 渲染同公式。
/// position 范围＝[`ScrollbarGeometry::max_top_row`]（最大可滚动顶行）。
/// 返回 `None`＝内容不超过一屏（不渲染、不可命中）。
fn thumb_layout(geo: ScrollbarGeometry) -> Option<(usize, usize)> {
    let track = geo.viewport_rows;
    if track == 0 || geo.total_rows <= geo.viewport_rows {
        return None;
    }
    // 前置保证 total > viewport ≥ 1 → max_position ≥ 1，分母恒非零
    let max_viewport_position = geo.max_top_row() + geo.viewport_rows;
    let thumb_len = rounding_divide(geo.viewport_rows * track, max_viewport_position)
        .clamp(1, track);
    let thumb_start = rounding_divide(geo.top_row * track, max_viewport_position)
        .clamp(0, track.saturating_sub(thumb_len));
    Some((thumb_start, thumb_len))
}

impl TranscriptView {
    /// 与渲染同帧的几何快照（须在 `render` 之后或事件处理时调用；布局走
    /// `LayoutCache`，宽度未变时为 O(n) 整数求和，不触发重折行）。
    fn scrollbar_geometry(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        cache: &mut RowCache,
    ) -> Option<ScrollbarGeometry> {
        if self.area.is_empty() {
            return None;
        }
        let snapshot = self.snapshot_cells();
        let cells = snapshot.as_deref().unwrap_or(cells);
        let has_live = self.snapshot().map_or(self.live.is_some(), |snapshot| {
            snapshot.pinned.contains_key(&EntryKey::Live)
        });
        // 已提交条目走行数缓存；live 尾部逐帧现算（EntryKey::Live 语义）
        let mut total_rows: usize = (0..cells.len()).map(|i| cache.rows(self, cells, i)).sum();
        if has_live {
            total_rows += self
                .layout(cells, cells.len())
                .map_or(/*default*/ 0, |l| l.row_count());
        }
        let (index, row) = self.start(cells);
        let above: usize = (0..index).map(|i| cache.rows(self, cells, i)).sum();
        Some(ScrollbarGeometry {
            total_rows,
            top_row: above + row,
            viewport_rows: usize::from(self.area.height),
        })
    }

    /// 滚动到绝对顶行：等价于对当前顶行做 `scroll(delta)`——完全复用既有钳制
    /// （顶部限位）与跟随语义（越过底部＝恢复 Latest 跟随），不另写滚动算法。
    fn scroll_to_absolute_row(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        cache: &mut RowCache,
        abs_row: usize,
    ) {
        if let Some(geo) = self.scrollbar_geometry(cells, cache) {
            let delta = abs_row as isize - geo.top_row as isize;
            if delta != 0 {
                self.scroll(cells, delta);
            }
        }
    }
}

/// 渲染沟位（draw 闭包内、`transcript.render` 之后调用：几何与渲染同帧，坑 3）。
/// 滚动条只画在**正文区**行上（`view.area`——prompt header 预留行不属轨道，渲染
/// 与命中共用该基准）；内容不超过一屏时留空（K1/A1）。
pub(crate) fn render_gutter(
    view: &mut TranscriptView,
    cells: &[Arc<dyn HistoryCell>],
    cache: &mut RowCache,
    gutter: Rect,
    buf: &mut Buffer,
) {
    if gutter.width == 0 || gutter.height == 0 {
        return;
    }
    // 正文区（view.area）⊂沟位区：只画正文行（header 预留行不属轨道）
    let body = view.area;
    let bar = Rect { y: body.y, height: body.height, ..gutter };
    if bar.width == 0 || bar.height == 0 {
        return;
    }
    let Some(geo) = view.scrollbar_geometry(cells, cache) else {
        return;
    };
    if geo.total_rows <= geo.viewport_rows {
        return;
    }
    let mut state = ScrollbarState::new(geo.max_top_row() + 1)
        .position(geo.top_row)
        .viewport_content_length(geo.viewport_rows);
    Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("║"))
        .thumb_symbol("█")
        .track_style(Style::new().dim())
        .render(bar, buf, &mut state);
}

/// 沟位按下（`pointer_row` 为**屏幕绝对行号**；正文区外的行——如 prompt header
/// 预留行——一律无操作）：
/// - 命中滑块 → 进入拖动（返回抓取状态）；
/// - 命中滑块上方/下方轨道 → 整页滚动（视口行数−1，与 PageUp/PageDown 语义一致，
///   K5），可重复点击连续翻页；
/// - 内容不足一屏 → 无操作。
pub(crate) fn gutter_press(
    view: &mut TranscriptView,
    cells: &[Arc<dyn HistoryCell>],
    cache: &mut RowCache,
    pointer_row: u16,
) -> Option<ScrollDrag> {
    let geo = view.scrollbar_geometry(cells, cache)?;
    let (thumb_start, thumb_len) = thumb_layout(geo)?;
    let body_y = view.area.y;
    if pointer_row < body_y {
        return None;
    }
    let row = usize::from(pointer_row - body_y);
    if row >= geo.viewport_rows {
        return None;
    }
    if (thumb_start..thumb_start + thumb_len).contains(&row) {
        return Some(ScrollDrag {
            grab_offset: (row - thumb_start) as u16,
        });
    }
    let page = geo.viewport_rows.saturating_sub(/*overlap*/ 1).max(1) as isize;
    let delta = if row < thumb_start { -page } else { page };
    view.scroll(cells, delta);
    None
}

/// 拖动映射：由指针位置反推滑块区间（减去抓取偏移、钳制在轨道内），再按渲染
/// 公式（`thumb_start = round(top × track / max_vpos)`）的逆映射换算目标顶行。
/// 端点精确：指针到轨道顶＝顶行 0；拇指贴轨道最底位＝最大可滚动顶行（量化
/// 折叠，scroll 的越过底部语义恢复跟随）。每次事件用当前几何，live 尾部增长
/// 自然生效。
pub(crate) fn drag_to(
    view: &mut TranscriptView,
    cells: &[Arc<dyn HistoryCell>],
    cache: &mut RowCache,
    drag: &ScrollDrag,
    pointer_row: u16,
) {
    let Some(geo) = view.scrollbar_geometry(cells, cache) else {
        return;
    };
    let Some((_, thumb_len)) = thumb_layout(geo) else {
        return;
    };
    let track = geo.viewport_rows;
    // 绝对行号→轨道行（正文区基准）；拖动期间指针越区按钳制处理
    let bottom_thumb_start = track.saturating_sub(thumb_len);
    let pointer = usize::from(pointer_row.saturating_sub(view.area.y)).min(track - 1);
    let thumb_start = pointer
        .saturating_sub(usize::from(drag.grab_offset))
        .min(bottom_thumb_start);
    let max_position = geo.max_top_row();
    let max_viewport_position = max_position + geo.viewport_rows;
    // 拇指贴到轨道最底位（量化后多个顶行映射同一拇指位）→ 折叠到最大可滚动顶行
    // （scroll 越过底部语义恢复跟随）；对称端 thumb_start=0 时逆映射本就精确为 0。
    let target = if thumb_start >= bottom_thumb_start {
        max_position
    } else {
        rounding_divide(thumb_start * max_viewport_position, track).min(max_position)
    };
    view.scroll_to_absolute_row(cells, cache, target);
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;


    fn plain(text: String) -> Arc<dyn HistoryCell> {
        Arc::new(crate::history_cell::PlainHistoryCell::new(vec![
            ratatui::text::Line::from(text),
        ]))
    }

    fn plain2(texts: Vec<&str>) -> Arc<dyn HistoryCell> {
        Arc::new(crate::history_cell::PlainHistoryCell::new(
            texts
                .into_iter()
                .map(|t| ratatui::text::Line::from(t.to_string()))
                .collect(),
        ))
    }

    fn cells(n: usize) -> Vec<Arc<dyn HistoryCell>> {
        (0..n).map(|i| plain(format!("S{i:03}"))).collect::<Vec<_>>()
    }

    /// 恰好 100 行正文：cell0 单行（无前分隔＝1 行）、cell1 两行文本（3 行）、
    /// 其余 48 个单行 cell（正文＋分隔＝2 行）——1+3+48×2=100。
    fn cells_100_rows() -> Vec<Arc<dyn HistoryCell>> {
        let mut cs = vec![plain("S000".into()), plain2(vec!["S001a", "S001b"])];
        cs.extend((2..50).map(|i| plain(format!("S{i:03}"))));
        cs
    }

    fn rendered_view_with(
        width: u16,
        height: u16,
        cs: Vec<Arc<dyn HistoryCell>>,
    ) -> (TranscriptView, RowCache, Vec<Arc<dyn HistoryCell>>, Buffer) {
        let mut view = TranscriptView::default();
        let mut cache = RowCache::default();
        let area = Rect::new(/*x*/ 0, /*y*/ 0, width, height);
        let mut buffer = Buffer::empty(area);
        view.render(area, &mut buffer, &cs);
        (view, cache, cs, buffer)
    }

    fn rendered_view(
        width: u16,
        height: u16,
        n: usize,
    ) -> (TranscriptView, RowCache, Vec<Arc<dyn HistoryCell>>, Buffer) {
        rendered_view_with(width, height, cells(n))
    }

    /// 跟随态：顶行＋视口＝总行数（贴底）；拇指贴底。
    #[test]
    fn geometry_at_bottom_sums_to_total() {
        let (mut view, mut cache, cs, _) = rendered_view(/*w*/ 20, /*h*/ 4, /*cells*/ 10);
        let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        assert_eq!(geo.viewport_rows, 4);
        assert_eq!(geo.top_row + geo.viewport_rows, geo.total_rows, "跟随态贴底");
        assert!(geo.total_rows >= 10, "每 cell 至少 1 行：{geo:?}");
        let (thumb_start, thumb_len) = thumb_layout(geo).unwrap();
        assert_eq!(thumb_start + thumb_len, 4, "拇指贴底：{thumb_start}+{thumb_len}");
    }

    /// 滚到顶：顶行＝0；拇指贴顶。
    #[test]
    fn geometry_at_top_and_absolute_scroll_clamps() {
        let (mut view, mut cache, cs, _) = rendered_view(20, 4, 10);
        view.scroll_to_absolute_row(&cs, &mut cache, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 4));
        view.render(Rect::new(0, 0, 20, 4), &mut buf, &cs);
        let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        assert_eq!(geo.top_row, 0, "滚到顶后顶行为 0");
        let (thumb_start, _) = thumb_layout(geo).unwrap();
        assert_eq!(thumb_start, 0, "拇指贴顶");

        // 越界目标被既有钳制吸收：超底恢复跟随（超顶钳制已在上一段断言）
        let total = view.scrollbar_geometry(&cs, &mut cache).unwrap().total_rows;
        view.scroll_to_absolute_row(&cs, &mut cache, total.saturating_add(100));
        view.render(Rect::new(0, 0, 20, 4), &mut buf, &cs);
        let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        assert!(view.is_following(), "拖到底恢复跟随");
        assert_eq!(geo.top_row + geo.viewport_rows, geo.total_rows);
    }

    /// 拇指命中公式与 ratatui 实渲染一致（同一 buffer 内滑块行）。
    #[test]
    fn thumb_layout_matches_rendered_scrollbar() {
        let (mut view, mut cache, cs, _) = rendered_view(20, 6, 30);
        let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        let (thumb_start, thumb_len) = thumb_layout(geo).unwrap();
        let gutter = Rect::new(/*x*/ 20, /*y*/ 0, /*w*/ 1, /*h*/ 6);
        let mut buf = Buffer::empty(Rect::new(0, 0, 21, 6));
        render_gutter(&mut view, &cs, &mut cache, gutter, &mut buf);
        let thumbs: Vec<usize> = (0..6)
            .filter(|&y| buf[(20, y as u16)].symbol() == "█")
            .collect();
        assert_eq!(thumbs, (thumb_start..thumb_start + thumb_len).collect::<Vec<_>>(),
            "命中公式与渲染一致");
    }

    /// 轨道点击＝整页（视口−1）；命中滑块＝拖动；短内容无操作。
    #[test]
    fn gutter_press_pages_on_track_and_grabs_thumb() {
        let (mut view, mut cache, cs, mut buf) = rendered_view(20, 4, 12);
        let geo0 = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        let (thumb_start, _) = thumb_layout(geo0).unwrap();
        assert!(thumb_start + /*thumb_len 至少 1*/ 1 <= 4);
        // 点击顶部轨道（滑块贴底时第 0 行必为轨道）→ 上翻一页
        let drag = gutter_press(&mut view, &cs, &mut cache, 0);
        assert!(drag.is_none(), "轨道点击不进入拖动");
        view.render(Rect::new(0, 0, 20, 4), &mut buf, &cs);
        let geo1 = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        assert_eq!(geo1.top_row, geo0.top_row.saturating_sub(3), "整页（视口−1）上翻");
        // 命中滑块 → 拖动状态（抓取偏移在滑块内）
        let (ts, tl) = thumb_layout(geo1).unwrap();
        let drag = gutter_press(&mut view, &cs, &mut cache, u16::try_from(ts + tl - 1).unwrap()).unwrap();
        // 指针拖到轨道顶 → 内容顶行归零（端点精确）
        drag_to(&mut view, &cs, &mut cache, &drag, 0);
        view.render(Rect::new(0, 0, 20, 4), &mut buf, &cs);
        assert_eq!(view.scrollbar_geometry(&cs, &mut cache).unwrap().top_row, 0, "拖到顶");
    }

    /// 短历史（不足一屏）：无拇指布局、沟位渲染留空、按下无操作。
    #[test]
    fn short_content_renders_blank_and_press_is_noop() {
        let (mut view, mut cache, cs, _) = rendered_view(20, 6, 2);
        let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        assert!(geo.total_rows <= 6, "夹具前置：不足一屏");
        assert!(thumb_layout(geo).is_none());
        let gutter = Rect::new(20, 0, 1, 6);
        let mut buf = Buffer::empty(Rect::new(0, 0, 21, 6));
        render_gutter(&mut view, &cs, &mut cache, gutter, &mut buf);
        assert!(buf[(20, 0)].symbol().trim().is_empty() && buf[(20, 5)].symbol().trim().is_empty(),
            "沟位留空");
        assert!(gutter_press(&mut view, &cs, &mut cache, 0).is_none());
    }


    /// 退化尺寸（A7 无 panic 注入面）：宽 1（内容宽 0）／高 1／空区域下渲染、
    /// 按下、拖动全部安全无 panic、无越界。
    #[test]
    fn degenerate_sizes_render_press_drag_no_panic() {
        for (w, h) in [(1u16, 10u16), (2, 1), (1, 1), (0, 5), (5, 0)] {
            let (mut view, mut cache, cs, _) = rendered_view(w, h, 8);
            let gutter = Rect { x: w, y: 0, width: 1.max(w), height: h };
            let area = Rect::new(0, 0, w + 1, h);
            let mut buf = Buffer::empty(area);
            render_gutter(&mut view, &cs, &mut cache, gutter, &mut buf);
            let _ = gutter_press(&mut view, &cs, &mut cache, 0);
            let drag = ScrollDrag { grab_offset: 0 };
            drag_to(&mut view, &cs, &mut cache, &drag, h.saturating_sub(1));
        }
    }

    /// 端点映射（011 收尾验收例：正文 100 行、可见 20 行）：
    /// 顶行 0 → 滑块贴顶；顶行 80（=总行数−视口，最大可滚动顶行）→ 滑块末端
    /// 贴轨道底部；滑块长度反映可见占比（20/100×轨道 20 = 4 行）。
    /// 防退化：position 范围是「最大可滚动顶行」而非正文总行数。
    #[test]
    fn thumb_endpoints_touch_track_ends_at_extremes() {
        let (mut view, mut cache, cs, mut buf) = rendered_view_with(20, 20, cells_100_rows());
        let area = Rect::new(0, 0, 20, 20);
        // 顶：顶行 0 → (start, len) = (0, 4)
        view.scroll_to_absolute_row(&cs, &mut cache, 0);
        view.render(area, &mut buf, &cs);
        let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        assert_eq!((geo.total_rows, geo.viewport_rows, geo.top_row), (100, 20, 0));
        assert_eq!(
            thumb_layout(geo).unwrap(),
            (0, 4),
            "顶行 0：滑块贴顶，长度＝可见占比"
        );
        // 底：顶行 80（跟随态）→ 滑块末端贴轨道底（start+len = 轨道 20）
        view.scroll_to_absolute_row(&cs, &mut cache, 80);
        view.render(area, &mut buf, &cs);
        let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        assert_eq!(geo.top_row, 80, "贴底顶行＝总行数−视口");
        assert!(view.is_following(), "到达底部恢复跟随");
        let (start, len) = thumb_layout(geo).unwrap();
        assert_eq!(
            start + len,
            20,
            "贴底：滑块末端触轨道底（{start}+{len}）"
        );
        // 性质扫查：顶贴顶、底贴底在常见几何下同时成立（行数取夹具实际值）
        for &(n_cells, viewport) in &[
            (4usize, 3usize),
            (7, 4),
            (6, 10),
            (8, 10),
            (16, 6),
            (51, 3),
            (200, 6),
        ] {
            let (mut view, mut cache, cs, mut buf) =
                rendered_view(20, u16::try_from(viewport).unwrap(), n_cells);
            let area = Rect::new(0, 0, 20, u16::try_from(viewport).unwrap());
            let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
            assert!(
                geo.total_rows > geo.viewport_rows,
                "夹具前置超一屏：{geo:?}"
            );
            let max_top = geo.total_rows - geo.viewport_rows;
            view.scroll_to_absolute_row(&cs, &mut cache, max_top);
            view.render(area, &mut buf, &cs);
            let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
            assert_eq!(geo.top_row, max_top, "cells={n_cells} viewport={viewport}");
            assert!(view.is_following(), "到达底部恢复跟随");
            let (start, len) = thumb_layout(geo).unwrap();
            assert_eq!(
                start + len,
                viewport,
                "贴底滑块末端触轨道底：cells={n_cells} viewport={viewport} ({start}+{len})"
            );
        }
    }

    /// 真实 Buffer 端点：贴顶时首行是 █、贴底时末行是 █（滑块触达轨道两端）。
    #[test]
    fn rendered_thumb_reaches_both_track_ends() {
        let (mut view, mut cache, cs, mut buf) = rendered_view_with(20, 20, cells_100_rows());
        let area = Rect::new(0, 0, 20, 20);
        let gutter = Rect::new(20, 0, 1, 20);
        let full = Rect::new(0, 0, 21, 20);
        let thumbs = |buf: &Buffer| {
            (0..20usize)
                .filter(|&y| buf[(20, y as u16)].symbol() == "█")
                .collect::<Vec<_>>()
        };
        // 顶
        view.scroll_to_absolute_row(&cs, &mut cache, 0);
        view.render(area, &mut buf, &cs);
        let mut gbuf = Buffer::empty(full);
        render_gutter(&mut view, &cs, &mut cache, gutter, &mut gbuf);
        let top = thumbs(&gbuf);
        assert_eq!(top.first(), Some(&0), "贴顶：首轨道行是滑块：{top:?}");
        assert_eq!(top.last(), Some(&3), "贴顶滑块 4 行：{top:?}");
        // 底
        view.scroll_to_absolute_row(&cs, &mut cache, 80);
        view.render(area, &mut buf, &cs);
        let mut gbuf = Buffer::empty(full);
        render_gutter(&mut view, &cs, &mut cache, gutter, &mut gbuf);
        let bottom = thumbs(&gbuf);
        assert_eq!(
            bottom.last(),
            Some(&19),
            "贴底：末轨道行是滑块：{bottom:?}"
        );
        assert_eq!(bottom.first(), Some(&16), "贴底滑块 4 行：{bottom:?}");
    }

    /// 拖动端点精确＋中点成比例＋贴底恢复跟随（不同抓取位置）：
    /// 抓滑块首行与末行各自拖到顶/底/中点，端点逐值断言（非「底部附近」）。
    #[test]
    fn drag_endpoints_exact_and_middle_proportional() {
        let (mut view, mut cache, cs, mut buf) = rendered_view_with(20, 20, cells_100_rows());
        let area = Rect::new(0, 0, 20, 20);
        let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        let (thumb_start, thumb_len) = thumb_layout(geo).unwrap();
        assert_eq!((thumb_start, thumb_len), (16, 4), "贴底态滑块 16..20");

        // 抓滑块首行（grab_offset=0）：指针行＝滑块首行
        let drag = gutter_press(&mut view, &cs, &mut cache, 16).unwrap();
        // 拖到轨道顶 → 顶行 0
        drag_to(&mut view, &cs, &mut cache, &drag, 0);
        view.render(area, &mut buf, &cs);
        assert_eq!(view.scrollbar_geometry(&cs, &mut cache).unwrap().top_row, 0);
        // 指针到轨道中部（行 10）→ 顶行 50＝10×（100/20 每轨道行 5 行正文），比例映射
        drag_to(&mut view, &cs, &mut cache, &drag, 10);
        view.render(area, &mut buf, &cs);
        let mid_geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        assert_eq!(mid_geo.top_row, 50, "中点比例映射：轨道行↔正文 5:1");
        let (mid_start, _) = thumb_layout(mid_geo).unwrap();
        assert_eq!(mid_start, 10, "拖动后滑块回到指针处（同一几何）");
        // 拖到轨道底（行 19）→ 顶行 80、恢复跟随、显示最新（底行对齐）
        drag_to(&mut view, &cs, &mut cache, &drag, 19);
        view.render(area, &mut buf, &cs);
        let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        assert_eq!(geo.top_row, 80, "拖到底＝最大可滚动顶行");
        assert_eq!(geo.top_row + geo.viewport_rows, geo.total_rows, "视口底＝正文底");
        assert!(view.is_following(), "拖到底恢复跟随");
        let (end_start, end_len) = thumb_layout(geo).unwrap();
        assert_eq!(end_start + end_len, 20, "滑块末端贴轨道底");

        // 抓滑块末行（grab_offset=3）：抓取点跟随指针，端点仍精确
        let drag = gutter_press(&mut view, &cs, &mut cache, 19).unwrap();
        drag_to(&mut view, &cs, &mut cache, &drag, 0);
        view.render(area, &mut buf, &cs);
        assert_eq!(view.scrollbar_geometry(&cs, &mut cache).unwrap().top_row, 0);
        drag_to(&mut view, &cs, &mut cache, &drag, 19);
        view.render(area, &mut buf, &cs);
        let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        assert_eq!(geo.top_row, 80, "末行抓取拖到底仍到最大顶行");
        assert!(view.is_following(), "末行抓取拖到底恢复跟随");
    }

    /// 拖到底折叠回归（S36 机制）：非整除几何下逆映射原始值落在最大顶行之内
    /// （239 行/视口 20：拇指底位 18 逆映射＝215 < 219）——必须折叠到最大顶行
    /// 并恢复跟随，否则拖到底后新输出不再自动贴底。
    #[test]
    fn drag_to_bottom_folds_quantization_to_max_top_and_follows() {
        // 120 个单行 cell＝239 行（cell0 无前分隔），视口 20 → 最大顶行 219
        let (mut view, mut cache, cs, mut buf) = rendered_view(20, 20, 120);
        let area = Rect::new(0, 0, 20, 20);
        let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        assert_eq!((geo.total_rows, geo.viewport_rows), (239, 20));
        let max_top = geo.max_top_row();
        assert_eq!(max_top, 219);
        let (thumb_start, thumb_len) = thumb_layout(geo).unwrap();
        let bottom_thumb_start = 20 - thumb_len;
        assert_eq!(thumb_start, bottom_thumb_start, "跟随态拇指贴轨道底");
        // 逆映射原始值确在最大顶行之内（量化缺口，证明本几何确非整除）
        let raw = rounding_divide(
            bottom_thumb_start * (max_top + geo.viewport_rows),
            geo.viewport_rows,
        );
        assert!(raw < max_top, "夹具前置：逆映射原始值 {raw} < {max_top}");

        // 抓拇指首行拖到轨道底行 → 折叠到最大顶行＋恢复跟随
        let drag = gutter_press(&mut view, &cs, &mut cache, thumb_start as u16).unwrap();
        drag_to(&mut view, &cs, &mut cache, &drag, 19);
        view.render(area, &mut buf, &cs);
        let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        assert_eq!(geo.top_row, max_top, "拖到底折叠到最大顶行（非 {raw}）");
        assert!(view.is_following(), "恢复跟随");
    }

    /// 拖动映射单调：指针逐行下移时内容顶行不回退；指针越出轨道被钳制。
    #[test]
    fn drag_mapping_is_monotonic_and_clamped() {
        let (mut view, mut cache, cs, mut buf) = rendered_view(20, 5, 40);
        let geo = view.scrollbar_geometry(&cs, &mut cache).unwrap();
        let (ts, tl) = thumb_layout(geo).unwrap();
        let drag = gutter_press(&mut view, &cs, &mut cache, u16::try_from(ts + tl - 1).unwrap()).unwrap();
        let mut last = 0usize;
        for pointer in 0..=30u16 {
            drag_to(&mut view, &cs, &mut cache, &drag, pointer);
            view.render(Rect::new(0, 0, 20, 5), &mut buf, &cs);
            let top = view.scrollbar_geometry(&cs, &mut cache).unwrap().top_row;
            assert!(top >= last, "指针下移时顶行单调不回退：{top} < {last}");
            last = top;
        }
        // 越顶拖动被钳制在 0
        drag_to(&mut view, &cs, &mut cache, &drag, 0);
        view.render(Rect::new(0, 0, 20, 5), &mut buf, &cs);
        assert_eq!(view.scrollbar_geometry(&cs, &mut cache).unwrap().top_row, 0);
    }
}
