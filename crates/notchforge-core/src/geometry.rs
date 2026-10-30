//! Notch geometry and layout math.
//!
//! All rectangles use **AppKit global coordinates**: origin at the bottom-left
//! of the primary screen, y grows upwards. The macOS layer converts `NSScreen`
//! properties into [`ScreenInfo`]; everything else here is pure arithmetic and
//! unit-tested.
//!
//! ## How the notch is found
//!
//! On notched MacBooks (macOS 12+) `NSScreen.safeAreaInsets.top` is the height
//! of the camera housing, and `auxiliaryTopLeftArea` / `auxiliaryTopRightArea`
//! are the usable menu-bar regions either side of it. The notch is exactly the
//! gap between those two areas. Screens without a notch get a *synthetic*
//! notch — a pill the height of the menu bar — so the app degrades gracefully.

use crate::config::{AppearanceConfig, ScreenChoice};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }
    pub fn max_x(&self) -> f64 {
        self.x + self.width
    }
    pub fn max_y(&self) -> f64 {
        self.y + self.height
    }
    pub fn mid_x(&self) -> f64 {
        self.x + self.width / 2.0
    }
    pub fn mid_y(&self) -> f64 {
        self.y + self.height / 2.0
    }
    pub fn size(&self) -> Size {
        Size { width: self.width, height: self.height }
    }
    /// Half-open containment so adjacent screens never both claim a point.
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.max_x() && p.y >= self.y && p.y < self.max_y()
    }
    /// Containment that includes the top edge, needed because the pointer at the
    /// very top of a screen reports `y == max_y`.
    pub fn contains_inclusive(&self, p: Point) -> bool {
        p.x >= self.x && p.x <= self.max_x() && p.y >= self.y && p.y <= self.max_y()
    }
    pub fn inset(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.x + dx, self.y + dy, (self.width - 2.0 * dx).max(0.0), (self.height - 2.0 * dy).max(0.0))
    }
    /// Grow by individual margins (negative values shrink).
    pub fn outset(&self, left: f64, right: f64, bottom: f64, top: f64) -> Rect {
        Rect::new(self.x - left, self.y - bottom, self.width + left + right, self.height + bottom + top)
    }
    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
}

/// What the macOS layer knows about one display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenInfo {
    /// `CGDirectDisplayID`.
    pub id: u32,
    pub frame: Rect,
    pub visible_frame: Rect,
    /// `safeAreaInsets.top` (0 on screens without a notch).
    pub safe_area_top: f64,
    pub aux_top_left: Option<Rect>,
    pub aux_top_right: Option<Rect>,
    pub backing_scale: f64,
    pub is_builtin: bool,
    /// Hosts the menu bar.
    pub is_main: bool,
}

impl ScreenInfo {
    pub fn has_notch(&self) -> bool {
        self.safe_area_top > 0.0 && self.aux_top_left.is_some() && self.aux_top_right.is_some()
    }

    /// Height of the menu bar, or a sensible default when it auto-hides.
    pub fn menu_bar_height(&self) -> f64 {
        let h = self.frame.max_y() - self.visible_frame.max_y();
        if h >= 20.0 { h.min(44.0) } else { 24.0 }
    }
}

/// The notch (real or synthetic) on a given screen.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Notch {
    /// Global rect of the camera housing (or synthetic pill).
    pub rect: Rect,
    /// `false` for the synthetic pill on non-notched displays.
    pub physical: bool,
}

/// Width of the synthetic pill shown on screens without a notch.
pub const SYNTHETIC_NOTCH_WIDTH: f64 = 196.0;

pub fn detect_notch(screen: &ScreenInfo) -> Notch {
    if let (true, Some(left), Some(right)) = (screen.has_notch(), screen.aux_top_left, screen.aux_top_right) {
        let x = left.max_x();
        let width = right.x - x;
        if width > 40.0 {
            let height = screen.safe_area_top;
            return Notch {
                rect: Rect::new(x, screen.frame.max_y() - height, width, height),
                physical: true,
            };
        }
    }
    let height = screen.menu_bar_height();
    let width = SYNTHETIC_NOTCH_WIDTH.min(screen.frame.width * 0.5);
    Notch {
        rect: Rect::new(screen.frame.mid_x() - width / 2.0, screen.frame.max_y() - height, width, height),
        physical: false,
    }
}

/// Pick the screen that hosts the UI.
pub fn choose_screen(screens: &[ScreenInfo], choice: ScreenChoice, mouse: Option<Point>) -> Option<usize> {
    if screens.is_empty() {
        return None;
    }
    let main = screens.iter().position(|s| s.is_main).unwrap_or(0);
    let picked = match choice {
        ScreenChoice::Notch => screens
            .iter()
            .position(ScreenInfo::has_notch)
            .or_else(|| screens.iter().position(|s| s.is_builtin)),
        ScreenChoice::Main => Some(main),
        ScreenChoice::Mouse => mouse.and_then(|p| screens.iter().position(|s| s.frame.contains_inclusive(p))),
    };
    Some(picked.unwrap_or(main))
}

/// The visual states the island can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LayoutKind {
    Collapsed,
    /// Collapsed with live indicators either side of the notch.
    CollapsedWithWings,
    /// A notification banner dropping out of the notch.
    Peek,
    /// Hover state.
    Compact,
    /// Pinned full panel.
    Expanded,
}

/// Visual parameters of the island for one layout.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IslandShape {
    pub width: f64,
    pub height: f64,
    /// Radius of the bottom corners (top corners are square, merging with the
    /// screen edge like the hardware notch).
    pub corner_radius: f64,
}

/// Width of a single wing in [`LayoutKind::CollapsedWithWings`].
pub const WING_WIDTH: f64 = 64.0;
/// Space around the largest island inside the window, for the shadow.
pub const WINDOW_MARGIN: f64 = 24.0;

/// Computes every rectangle the UI needs for one screen.
#[derive(Debug, Clone, PartialEq)]
pub struct NotchLayout {
    pub screen: ScreenInfo,
    pub notch: Notch,
    pub appearance: AppearanceConfig,
}

impl NotchLayout {
    pub fn new(screen: ScreenInfo, appearance: AppearanceConfig) -> Self {
        let notch = detect_notch(&screen);
        Self { screen, notch, appearance }
    }

    pub fn shape(&self, kind: LayoutKind) -> IslandShape {
        let n = self.notch.rect;
        let a = &self.appearance;
        // Radius of the hardware notch's lower corners is roughly a third of its height.
        let base_radius = (n.height * 0.38).clamp(6.0, 14.0);
        match kind {
            LayoutKind::Collapsed => IslandShape { width: n.width, height: n.height, corner_radius: base_radius },
            LayoutKind::CollapsedWithWings => IslandShape {
                width: n.width + 2.0 * WING_WIDTH,
                height: n.height,
                corner_radius: base_radius,
            },
            LayoutKind::Peek => IslandShape {
                width: (n.width + 2.0 * WING_WIDTH + 80.0).max(380.0).min(a.compact_width),
                height: n.height + 52.0,
                corner_radius: 20.0,
            },
            LayoutKind::Compact => IslandShape {
                width: a.compact_width.max(n.width + 2.0 * WING_WIDTH),
                height: n.height + a.compact_height,
                corner_radius: 24.0,
            },
            LayoutKind::Expanded => IslandShape {
                width: a.expanded_width.max(n.width + 2.0 * WING_WIDTH),
                height: n.height + a.expanded_height,
                corner_radius: 30.0,
            },
        }
    }

    /// The panel window: big enough for the expanded island plus margins,
    /// top-aligned to the screen and centred on the notch.
    pub fn window_frame(&self) -> Rect {
        let max = self.shape(LayoutKind::Expanded);
        let width = max.width + 2.0 * WINDOW_MARGIN;
        let height = max.height + WINDOW_MARGIN;
        let mut x = self.notch.rect.mid_x() - width / 2.0;
        // Keep the window on its screen even if the notch were off-centre.
        x = x.clamp(self.screen.frame.x, (self.screen.frame.max_x() - width).max(self.screen.frame.x));
        Rect::new(x, self.screen.frame.max_y() - height, width, height)
    }

    /// Island rect in *window-local* coordinates for a given width/height,
    /// horizontally centred on the notch and pinned to the top edge.
    pub fn island_in_window(&self, width: f64, height: f64) -> Rect {
        let win = self.window_frame();
        let cx = self.notch.rect.mid_x() - win.x;
        Rect::new(cx - width / 2.0, win.height - height, width, height)
    }

    /// Island rect in global coordinates.
    pub fn island_global(&self, width: f64, height: f64) -> Rect {
        let win = self.window_frame();
        let local = self.island_in_window(width, height);
        Rect::new(local.x + win.x, local.y + win.y, local.width, local.height)
    }

    /// Zone that arms hover while collapsed: the notch, a little wider, plus a
    /// strip reaching the very top pixel row (the pointer can sit *at* the edge).
    pub fn activation_zone(&self) -> Rect {
        self.notch.rect.outset(18.0, 18.0, 6.0, 1.0)
    }

    /// Zone the pointer must leave before the hover state starts collapsing.
    pub fn retention_zone(&self, kind: LayoutKind) -> Rect {
        let s = self.shape(kind);
        self.island_global(s.width, s.height).outset(14.0, 14.0, 14.0, 1.0)
    }

    /// Larger zone used while dragging files, so the drop target is easy to hit.
    pub fn drag_zone(&self) -> Rect {
        self.notch.rect.outset(120.0, 120.0, 60.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 14" MacBook Pro, default scaling: 1512×982 points, 32pt notch, 185pt wide.
    fn mbp14() -> ScreenInfo {
        ScreenInfo {
            id: 1,
            frame: Rect::new(0.0, 0.0, 1512.0, 982.0),
            visible_frame: Rect::new(0.0, 0.0, 1512.0, 950.0),
            safe_area_top: 32.0,
            aux_top_left: Some(Rect::new(0.0, 950.0, 663.5, 32.0)),
            aux_top_right: Some(Rect::new(848.5, 950.0, 663.5, 32.0)),
            backing_scale: 2.0,
            is_builtin: true,
            is_main: true,
        }
    }

    fn external() -> ScreenInfo {
        ScreenInfo {
            id: 2,
            frame: Rect::new(1512.0, 0.0, 2560.0, 1440.0),
            visible_frame: Rect::new(1512.0, 0.0, 2560.0, 1415.0),
            safe_area_top: 0.0,
            aux_top_left: None,
            aux_top_right: None,
            backing_scale: 1.0,
            is_builtin: false,
            is_main: false,
        }
    }

    #[test]
    fn detects_physical_notch() {
        let n = detect_notch(&mbp14());
        assert!(n.physical);
        assert_eq!(n.rect, Rect::new(663.5, 950.0, 185.0, 32.0));
        assert_eq!(n.rect.mid_x(), 756.0);
    }

    #[test]
    fn synthesizes_pill_without_notch() {
        let n = detect_notch(&external());
        assert!(!n.physical);
        assert_eq!(n.rect.height, 25.0);
        assert_eq!(n.rect.mid_x(), 1512.0 + 1280.0);
        assert_eq!(n.rect.max_y(), 1440.0);
    }

    #[test]
    fn auto_hidden_menu_bar_falls_back_to_default_height() {
        let mut s = external();
        s.visible_frame = s.frame;
        assert_eq!(s.menu_bar_height(), 24.0);
    }

    #[test]
    fn screen_choice() {
        let mut ext = external();
        ext.is_main = true;
        let mut builtin = mbp14();
        builtin.is_main = false;
        let screens = vec![ext, builtin];
        assert_eq!(choose_screen(&screens, ScreenChoice::Notch, None), Some(1));
        assert_eq!(choose_screen(&screens, ScreenChoice::Main, None), Some(0));
        assert_eq!(choose_screen(&screens, ScreenChoice::Mouse, Some(Point { x: 100.0, y: 100.0 })), Some(1));
        // Mouse outside every screen falls back to main.
        assert_eq!(choose_screen(&screens, ScreenChoice::Mouse, Some(Point { x: -5.0, y: 0.0 })), Some(0));
        assert_eq!(choose_screen(&[], ScreenChoice::Main, None), None);
    }

    #[test]
    fn notch_screen_choice_without_notch_uses_main() {
        let mut ext = external();
        ext.is_main = true;
        assert_eq!(choose_screen(&[ext], ScreenChoice::Notch, None), Some(0));
    }

    #[test]
    fn window_is_top_aligned_and_centred_on_notch() {
        let layout = NotchLayout::new(mbp14(), AppearanceConfig::default());
        let win = layout.window_frame();
        assert_eq!(win.max_y(), 982.0);
        assert!((win.mid_x() - 756.0).abs() < 1e-9);
        let island = layout.island_global(185.0, 32.0);
        assert_eq!(island, layout.notch.rect);
    }

    #[test]
    fn shapes_grow_monotonically() {
        let layout = NotchLayout::new(mbp14(), AppearanceConfig::default());
        let kinds = [LayoutKind::Collapsed, LayoutKind::CollapsedWithWings, LayoutKind::Compact, LayoutKind::Expanded];
        for pair in kinds.windows(2) {
            let (a, b) = (layout.shape(pair[0]), layout.shape(pair[1]));
            assert!(b.width >= a.width && b.height >= a.height, "{:?} -> {:?}", pair[0], pair[1]);
        }
        let win = layout.window_frame();
        let e = layout.shape(LayoutKind::Expanded);
        assert!(win.width >= e.width && win.height >= e.height);
    }

    #[test]
    fn activation_zone_covers_top_edge() {
        let layout = NotchLayout::new(mbp14(), AppearanceConfig::default());
        let z = layout.activation_zone();
        assert!(z.contains_inclusive(Point { x: 756.0, y: 982.0 }));
        assert!(z.contains_inclusive(Point { x: 650.0, y: 960.0 }));
        assert!(!z.contains_inclusive(Point { x: 756.0, y: 900.0 }));
    }

    #[test]
    fn window_stays_on_screen_for_offcentre_notch() {
        let mut s = external();
        s.frame = Rect::new(0.0, 0.0, 800.0, 600.0);
        s.visible_frame = Rect::new(0.0, 0.0, 800.0, 575.0);
        let layout = NotchLayout::new(s, AppearanceConfig::default());
        let win = layout.window_frame();
        assert!(win.x >= 0.0);
    }
}
