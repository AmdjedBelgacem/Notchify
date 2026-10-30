//! Small AppKit building blocks shared by the panel, menus and settings.
//!
//! * [`ActionTarget`] turns a Rust closure into an Objective-C target/action
//!   pair. AppKit holds targets *weakly*, so owners must keep the returned
//!   `Retained<ActionTarget>` alive (see [`Targets`]).
//! * [`FlippedView`] is a plain `NSView` with a top-left origin, which makes
//!   manual layout read naturally.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSButton, NSColor, NSControlSize, NSFont, NSFontWeight, NSFontWeightMedium, NSFontWeightRegular,
    NSFontWeightSemibold, NSImage, NSImageScaling, NSImageSymbolConfiguration, NSImageView, NSLineBreakMode,
    NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;

// ─────────────────────────────────────────────────────────────────────────────
// ActionTarget
// ─────────────────────────────────────────────────────────────────────────────

/// Callback invoked with the control that sent the action.
pub type ActionFn = Box<dyn Fn(Option<&AnyObject>)>;

pub struct ActionIvars {
    callback: ActionFn,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ActionIvars]
    pub struct ActionTarget;

    impl ActionTarget {
        #[unsafe(method(fire:))]
        fn fire(&self, sender: Option<&AnyObject>) {
            (self.ivars().callback)(sender);
        }
    }

    unsafe impl NSObjectProtocol for ActionTarget {}
);

impl ActionTarget {
    pub fn new(mtm: MainThreadMarker, f: impl Fn(Option<&AnyObject>) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ActionIvars { callback: Box::new(f) });
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }

    pub fn action() -> Sel {
        sel!(fire:)
    }

    pub fn as_any(&self) -> &AnyObject {
        self
    }
}

/// Keeps action targets alive for as long as the views using them.
///
/// Never call [`Targets::clear`] synchronously from inside one of its own
/// callbacks (that would free the running closure); defer it to a later
/// main-queue turn instead, as `app::show_settings_soon` does.
#[derive(Default)]
pub struct Targets(RefCell<Vec<Retained<ActionTarget>>>);

impl Targets {
    pub fn add(&self, mtm: MainThreadMarker, f: impl Fn(Option<&AnyObject>) + 'static) -> Retained<ActionTarget> {
        let t = ActionTarget::new(mtm, f);
        self.0.borrow_mut().push(t.clone());
        t
    }
    pub fn clear(&self) {
        self.0.borrow_mut().clear();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FlippedView
// ─────────────────────────────────────────────────────────────────────────────

define_class!(
    // SAFETY: NSView subclass without ivars or Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    pub struct FlippedView;

    impl FlippedView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    pub fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm);
        // SAFETY: NSView's designated initializer.
        unsafe { msg_send![this, initWithFrame: frame] }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Geometry & colour helpers
// ─────────────────────────────────────────────────────────────────────────────

pub fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

pub fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

pub fn rgba(r: f64, g: f64, b: f64, a: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a)
}

pub fn rgb8((r, g, b): (u8, u8, u8)) -> Retained<NSColor> {
    rgba(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, 1.0)
}

/// Text colours tuned for the black island.
pub struct Palette {
    pub primary: Retained<NSColor>,
    pub secondary: Retained<NSColor>,
    pub tertiary: Retained<NSColor>,
    pub accent: Retained<NSColor>,
    pub warning: Retained<NSColor>,
    pub success: Retained<NSColor>,
    pub card: Retained<NSColor>,
}

impl Palette {
    pub fn new(high_contrast: bool) -> Self {
        let (s, t) = if high_contrast { (0.85, 0.7) } else { (0.62, 0.42) };
        Self {
            primary: rgba(1.0, 1.0, 1.0, 0.96),
            secondary: rgba(1.0, 1.0, 1.0, s),
            tertiary: rgba(1.0, 1.0, 1.0, t),
            accent: NSColor::controlAccentColor(),
            warning: rgba(1.0, 0.62, 0.2, 1.0),
            success: rgba(0.3, 0.85, 0.45, 1.0),
            card: rgba(1.0, 1.0, 1.0, if high_contrast { 0.14 } else { 0.07 }),
        }
    }
}

pub fn weight(w: Weight) -> NSFontWeight {
    // SAFETY: reading AppKit's constant font weights.
    unsafe {
        match w {
            Weight::Regular => NSFontWeightRegular,
            Weight::Medium => NSFontWeightMedium,
            Weight::Semibold => NSFontWeightSemibold,
        }
    }
}

#[derive(Clone, Copy)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
}

pub fn font(size: f64, w: Weight) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, weight(w))
}

pub fn mono_digits(size: f64, w: Weight) -> Retained<NSFont> {
    NSFont::monospacedDigitSystemFontOfSize_weight(size, weight(w))
}

// ─────────────────────────────────────────────────────────────────────────────
// Controls
// ─────────────────────────────────────────────────────────────────────────────

/// Non-editable, non-selectable single- or multi-line label.
pub fn label(mtm: MainThreadMarker, text: &str, f: &NSFont, color: &NSColor, frame: NSRect) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&ns(text), mtm);
    l.setFont(Some(f));
    l.setTextColor(Some(color));
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    l.setFrame(frame);
    l
}

pub fn wrapping_label(
    mtm: MainThreadMarker,
    text: &str,
    f: &NSFont,
    color: &NSColor,
    frame: NSRect,
    lines: isize,
) -> Retained<NSTextField> {
    let l = NSTextField::wrappingLabelWithString(&ns(text), mtm);
    l.setFont(Some(f));
    l.setTextColor(Some(color));
    l.setMaximumNumberOfLines(lines);
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    l.setFrame(frame);
    l
}

pub fn set_text(l: &NSTextField, text: &str) {
    l.setStringValue(&ns(text));
}

pub fn symbol_image(name: &str, point_size: f64, accessibility: &str) -> Option<Retained<NSImage>> {
    let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), Some(&ns(accessibility)))?;
    let cfg = NSImageSymbolConfiguration::configurationWithPointSize_weight(point_size, weight(Weight::Medium));
    img.imageWithSymbolConfiguration(&cfg).or(Some(img))
}

pub fn symbol_view(
    mtm: MainThreadMarker,
    name: &str,
    point_size: f64,
    tint: &NSColor,
    frame: NSRect,
    accessibility: &str,
) -> Retained<NSImageView> {
    let v = NSImageView::new(mtm);
    v.setImage(symbol_image(name, point_size, accessibility).as_deref());
    v.setContentTintColor(Some(tint));
    v.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    v.setFrame(frame);
    v
}

pub fn set_symbol(v: &NSImageView, name: &str, point_size: f64, accessibility: &str) {
    v.setImage(symbol_image(name, point_size, accessibility).as_deref());
}

/// Borderless SF Symbol button with a tooltip and VoiceOver label.
pub fn icon_button(
    mtm: MainThreadMarker,
    symbol: &str,
    tooltip: &str,
    tint: &NSColor,
    target: &ActionTarget,
    frame: NSRect,
) -> Retained<NSButton> {
    let image = symbol_image(symbol, 13.0, tooltip);
    // SAFETY: valid target/selector; target is kept alive by the caller.
    let b = unsafe {
        match image {
            Some(img) => {
                NSButton::buttonWithImage_target_action(&img, Some(target.as_any()), Some(ActionTarget::action()), mtm)
            }
            None => NSButton::buttonWithTitle_target_action(
                &ns(tooltip),
                Some(target.as_any()),
                Some(ActionTarget::action()),
                mtm,
            ),
        }
    };
    b.setBordered(false);
    b.setContentTintColor(Some(tint));
    b.setToolTip(Some(&ns(tooltip)));
    set_accessibility_label(&b, tooltip);
    b.setFrame(frame);
    b
}

pub fn text_button(mtm: MainThreadMarker, title: &str, target: &ActionTarget, frame: NSRect) -> Retained<NSButton> {
    // SAFETY: valid target/selector; target is kept alive by the caller.
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(&ns(title), Some(target.as_any()), Some(ActionTarget::action()), mtm)
    };
    b.setControlSize(NSControlSize::Small);
    b.setFrame(frame);
    b
}

pub fn set_accessibility_label(view: &NSView, text: &str) {
    // SAFETY: setAccessibilityLabel: exists on every NSView (NSAccessibility protocol).
    let _: () = unsafe { msg_send![view, setAccessibilityLabel: &*ns(text)] };
}

/// Make a view layer-backed with a rounded "card" background.
pub fn make_card(view: &NSView, color: &NSColor, radius: f64) {
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setBackgroundColor(Some(&color.CGColor()));
        layer.setCornerRadius(radius);
    }
}

pub fn remove_all_subviews(view: &NSView) {
    for v in view.subviews().iter() {
        v.removeFromSuperview();
    }
}
