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

define_class!(
    // SAFETY: NSTextField subclass without ivars or Drop; only drawing changes.
    #[unsafe(super(NSTextField))]
    #[thread_kind = MainThreadOnly]
    pub struct TextLabel;

    impl TextLabel {
        /// Draw text without font smoothing (stem darkening), as the design
        /// boards render it: smoothing thickens light-on-dark text.
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, rect: NSRect) {
            if let Some(ctx) = objc2_app_kit::NSGraphicsContext::currentContext() {
                let cg = ctx.CGContext();
                objc2_core_graphics::CGContext::set_allows_font_smoothing(Some(&cg), false);
                objc2_core_graphics::CGContext::set_should_smooth_fonts(Some(&cg), false);
            }
            // SAFETY: forwarding the same message to NSTextField.
            unsafe {
                let _: () = msg_send![super(self), drawRect: rect];
            }
        }
    }
);

/// `+[NSTextField labelWithString:]` and friends allocate from the receiver,
/// so sending them to `TextLabel` yields fully configured labels of our class.
fn text_label_class() -> &'static objc2::runtime::AnyClass {
    use objc2::ClassType;
    TextLabel::class()
}

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
#[derive(Clone)]
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

/// AppKit draws a label's text 2 pt inside its frame (the text container's
/// line fragment padding). Widen the frame by that much on each side so text
/// starts exactly at the frame's x, as on the design boards (CSS).
pub const LABEL_INSET: f64 = 2.0;

fn text_frame(frame: NSRect) -> NSRect {
    NSRect::new(
        objc2_foundation::NSPoint::new(frame.origin.x - LABEL_INSET, frame.origin.y),
        objc2_foundation::NSSize::new(frame.size.width + 2.0 * LABEL_INSET, frame.size.height),
    )
}

/// Non-editable, non-selectable single- or multi-line label.
pub fn label(mtm: MainThreadMarker, text: &str, f: &NSFont, color: &NSColor, frame: NSRect) -> Retained<NSTextField> {
    let _ = mtm;
    // SAFETY: NSTextField's label factory, sent to our subclass.
    let l: Retained<NSTextField> = unsafe { msg_send![text_label_class(), labelWithString: &*ns(text)] };
    l.setFont(Some(f));
    l.setTextColor(Some(color));
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    l.setFrame(text_frame(frame));
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
    let _ = mtm;
    // SAFETY: NSTextField's wrapping label factory, sent to our subclass.
    let l: Retained<NSTextField> = unsafe { msg_send![text_label_class(), wrappingLabelWithString: &*ns(text)] };
    l.setFont(Some(f));
    l.setTextColor(Some(color));
    l.setMaximumNumberOfLines(lines);
    // Wrap by word; only the last visible line truncates. (A truncating line
    // break mode on the field itself would turn wrapping off.)
    l.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    if let Some(cell) = l.cell() {
        cell.setTruncatesLastVisibleLine(true);
    }
    l.setFrame(text_frame(frame));
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

/// Invisible button covering `frame`: makes a composite row clickable.
pub fn click_overlay(mtm: MainThreadMarker, target: &ActionTarget, frame: NSRect, label: &str) -> Retained<NSButton> {
    // SAFETY: valid target/selector; target is kept alive by the caller.
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(&ns(""), Some(target.as_any()), Some(ActionTarget::action()), mtm)
    };
    b.setBordered(false);
    b.setTransparent(true);
    b.setFrame(frame);
    set_accessibility_label(&b, label);
    b
}

/// [`icon_button`] with a custom symbol size.
pub fn symbol_button(
    mtm: MainThreadMarker,
    symbol: &str,
    point_size: f64,
    tooltip: &str,
    tint: &NSColor,
    target: &ActionTarget,
    frame: NSRect,
) -> Retained<NSButton> {
    let b = icon_button(mtm, symbol, tooltip, tint, target, frame);
    b.setImage(symbol_image(symbol, point_size, tooltip).as_deref());
    b
}

pub fn set_button_symbol(b: &NSButton, symbol: &str, point_size: f64, tooltip: &str) {
    b.setImage(symbol_image(symbol, point_size, tooltip).as_deref());
    b.setToolTip(Some(&ns(tooltip)));
    set_accessibility_label(b, tooltip);
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

// ─────────────────────────────────────────────────────────────────────────────
// Design-system controls (see the "Tokens & motion" board)
// ─────────────────────────────────────────────────────────────────────────────

/// Attributed title in one colour and font.
pub fn attributed(text: &str, f: &NSFont, color: &NSColor) -> Retained<objc2_foundation::NSAttributedString> {
    use objc2::AnyThread;
    use objc2_foundation::{NSAttributedString, NSDictionary};
    // SAFETY: AppKit's attribute-name constants.
    let (fk, ck) = unsafe { (objc2_app_kit::NSFontAttributeName, objc2_app_kit::NSForegroundColorAttributeName) };
    let fv: &AnyObject = f;
    let cv: &AnyObject = color;
    let dict = NSDictionary::<NSString, AnyObject>::from_slices(&[fk, ck], &[fv, cv]);
    // SAFETY: the attributes have the documented value types.
    unsafe { NSAttributedString::initWithString_attributes(NSAttributedString::alloc(), &ns(text), Some(&dict)) }
}

/// Borderless rounded button with an optional leading SF Symbol: the pill
/// buttons of the design ("Output", "Clear", segment items).
#[allow(clippy::too_many_arguments)]
pub fn pill_button(
    mtm: MainThreadMarker,
    title: &str,
    symbol: Option<&str>,
    f: &NSFont,
    fg: &NSColor,
    bg: &NSColor,
    radius: f64,
    target: &ActionTarget,
    frame: NSRect,
) -> Retained<NSButton> {
    let b = text_button(mtm, title, target, frame);
    b.setBordered(false);
    style_pill(&b, title, symbol, f, fg, bg, radius);
    b.setFrame(frame);
    b
}

/// Restyle a pill button (selection changes).
pub fn style_pill(
    b: &NSButton,
    title: &str,
    symbol: Option<&str>,
    f: &NSFont,
    fg: &NSColor,
    bg: &NSColor,
    radius: f64,
) {
    b.setAttributedTitle(&attributed(title, f, fg));
    if let Some(sym) = symbol {
        let img = symbol_image(sym, f.pointSize() - 0.5, title);
        if let Some(i) = &img {
            i.setTemplate(true);
        }
        b.setImage(img.as_deref());
        b.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageLeading);
        b.setImageHugsTitle(true);
    }
    b.setContentTintColor(Some(fg));
    make_card(b, bg, radius);
    set_accessibility_label(b, title);
}

/// Keep a wash/glow layer fully transparent across the top `solid_h` pt (the
/// notch band, which must stay pure black to blend with the hardware notch)
/// and fade it in over the next `ramp` pt. The layer's geometry is y-down
/// (it lives in a flipped view) and its frame must already be set.
pub fn fade_in_below(layer: &objc2_quartz_core::CALayer, solid_h: f64, ramp: f64) {
    let h = layer.bounds().size.height.max(1.0);
    let mask = objc2_quartz_core::CAGradientLayer::layer();
    mask.setFrame(layer.bounds());
    mask.setStartPoint(NSPoint::new(0.5, 0.0));
    mask.setEndPoint(NSPoint::new(0.5, 1.0));
    set_gradient(&mask, &[(0.0, 0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0)]);
    let at = |v: f64| objc2_foundation::NSNumber::new_f64((v / h).clamp(0.0, 1.0));
    let locations = [at(0.0), at(solid_h), at(solid_h + ramp)];
    mask.setLocations(Some(&objc2_foundation::NSArray::from_retained_slice(&locations)));
    // SAFETY: a plain gradient layer used as a mask.
    unsafe { layer.setMask(Some(&mask)) };
}

/// One concave flare, in a layer of `e` × `e` pt (y-up). `left` is the flare
/// on the island's left; its mirror is the right one. The shape is the
/// square minus a quarter disc centred on its outer-bottom corner: full width
/// along the top edge, narrowing to nothing `e` pt lower, like the real notch
/// where it joins the menu bar.
pub fn set_notch_flare(layer: &objc2_quartz_core::CAShapeLayer, e: f64, left: bool) {
    use objc2_core_graphics::CGMutablePath as P;
    let path = P::new();
    let half_pi = std::f64::consts::FRAC_PI_2;
    // SAFETY: null transforms are allowed; the path is owned by us.
    unsafe {
        if left {
            P::move_to_point(Some(&path), std::ptr::null(), 0.0, e);
            P::add_line_to_point(Some(&path), std::ptr::null(), e, e);
            P::add_line_to_point(Some(&path), std::ptr::null(), e, 0.0);
            P::add_arc(Some(&path), std::ptr::null(), 0.0, 0.0, e, 0.0, half_pi, false);
        } else {
            P::move_to_point(Some(&path), std::ptr::null(), e, e);
            P::add_line_to_point(Some(&path), std::ptr::null(), 0.0, e);
            P::add_line_to_point(Some(&path), std::ptr::null(), 0.0, 0.0);
            P::add_arc(Some(&path), std::ptr::null(), e, 0.0, e, std::f64::consts::PI, half_pi, true);
        }
        P::close_subpath(Some(&path));
    }
    layer.setPath(Some(&path));
}

/// Like [`attributed`], with `kern` pt of extra space after every character
/// (letter-spaced captions).
pub fn attributed_tracked(
    text: &str,
    f: &NSFont,
    color: &NSColor,
    kern: f64,
) -> Retained<objc2_foundation::NSAttributedString> {
    use objc2::AnyThread;
    use objc2_foundation::{NSAttributedString, NSDictionary, NSNumber};
    // SAFETY: AppKit's attribute-name constants.
    let (fk, ck, kk) = unsafe {
        (
            objc2_app_kit::NSFontAttributeName,
            objc2_app_kit::NSForegroundColorAttributeName,
            objc2_app_kit::NSKernAttributeName,
        )
    };
    let fv: &AnyObject = f;
    let cv: &AnyObject = color;
    let n = NSNumber::new_f64(kern);
    let kv: &AnyObject = &n;
    let dict = NSDictionary::<NSString, AnyObject>::from_slices(&[fk, ck, kk], &[fv, cv, kv]);
    // SAFETY: the attributes have the documented value types.
    unsafe { NSAttributedString::initWithString_attributes(NSAttributedString::alloc(), &ns(text), Some(&dict)) }
}

/// Run `f` without Core Animation's implicit 0.25 s animations.
pub fn without_animation(f: impl FnOnce()) {
    objc2_quartz_core::CATransaction::begin();
    objc2_quartz_core::CATransaction::setDisableActions(true);
    f();
    objc2_quartz_core::CATransaction::commit();
}

/// A gradient layer from `(r, g, b, a)` stops (sRGB 0–1).
pub fn gradient_layer(
    stops: &[(f64, f64, f64, f64)],
    start: NSPoint,
    end: NSPoint,
) -> Retained<objc2_quartz_core::CAGradientLayer> {
    let g = objc2_quartz_core::CAGradientLayer::layer();
    set_gradient(&g, stops);
    g.setStartPoint(start);
    g.setEndPoint(end);
    g
}

pub fn set_gradient(g: &objc2_quartz_core::CAGradientLayer, stops: &[(f64, f64, f64, f64)]) {
    let colors: Vec<_> = stops.iter().map(|(r, gg, b, a)| rgba(*r, *gg, *b, *a).CGColor()).collect();
    // SAFETY: CGColor is toll-free bridged to an Objective-C object.
    let objs: Vec<&AnyObject> =
        colors.iter().map(|c| unsafe { &*(&**c as *const objc2_core_graphics::CGColor as *const AnyObject) }).collect();
    let arr = objc2_foundation::NSArray::from_slice(&objs);
    // SAFETY: an array of CGColors, as CAGradientLayer expects.
    unsafe { g.setColors(Some(&arr)) };
}

/// Callback with the slider's value (0–1).
pub type SliderFn = Box<dyn Fn(f64)>;

pub struct SliderIvars {
    value: std::cell::Cell<f64>,
    knob: bool,
    on_change: RefCell<Option<SliderFn>>,
    on_commit: RefCell<Option<SliderFn>>,
    tracking: std::cell::Cell<bool>,
    track: Retained<objc2_quartz_core::CALayer>,
    fill: Retained<objc2_quartz_core::CALayer>,
    knob_layer: Retained<objc2_quartz_core::CALayer>,
}

define_class!(
    /// The design's thin slider: a 4 pt track, a white fill and an optional
    /// 14 pt knob (volume, brightness) or none (playback progress).
    // SAFETY: NSView subclass; ivars are plain Rust values; no Drop impl.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = SliderIvars]
    pub struct ThinSlider;

    impl ThinSlider {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&objc2_app_kit::NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &objc2_app_kit::NSEvent) {
            self.ivars().tracking.set(true);
            self.track_event(event, false);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &objc2_app_kit::NSEvent) {
            self.track_event(event, false);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &objc2_app_kit::NSEvent) {
            self.track_event(event, true);
            self.ivars().tracking.set(false);
        }
    }
);

impl ThinSlider {
    pub fn new(mtm: MainThreadMarker, frame: NSRect, knob: bool, fill: &NSColor) -> Retained<Self> {
        let track = objc2_quartz_core::CALayer::new();
        let fill_layer = objc2_quartz_core::CALayer::new();
        let knob_layer = objc2_quartz_core::CALayer::new();
        track.setBackgroundColor(Some(&rgba(1.0, 1.0, 1.0, 0.16).CGColor()));
        track.setCornerRadius(2.0);
        fill_layer.setBackgroundColor(Some(&fill.CGColor()));
        fill_layer.setCornerRadius(2.0);
        knob_layer.setBackgroundColor(Some(&NSColor::whiteColor().CGColor()));
        knob_layer.setCornerRadius(7.0);
        knob_layer.setShadowOpacity(0.5);
        knob_layer.setShadowRadius(2.0);
        knob_layer.setShadowOffset(NSSize::new(0.0, 1.0));
        knob_layer.setHidden(!knob);
        let this = Self::alloc(mtm).set_ivars(SliderIvars {
            value: std::cell::Cell::new(0.0),
            knob,
            on_change: RefCell::new(None),
            on_commit: RefCell::new(None),
            tracking: std::cell::Cell::new(false),
            track,
            fill: fill_layer,
            knob_layer,
        });
        // SAFETY: NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setWantsLayer(true);
        if let Some(root) = this.layer() {
            let iv = this.ivars();
            root.addSublayer(&iv.track);
            root.addSublayer(&iv.fill);
            root.addSublayer(&iv.knob_layer);
        }
        // SAFETY: setAccessibilityRole: exists on every NSView.
        let _: () = unsafe { msg_send![&*this, setAccessibilityRole: &*ns("AXSlider")] };
        this.layout_layers();
        this
    }

    /// Called continuously while dragging.
    pub fn set_on_change(&self, f: impl Fn(f64) + 'static) {
        *self.ivars().on_change.borrow_mut() = Some(Box::new(f));
    }

    /// Called once when the drag ends.
    pub fn set_on_commit(&self, f: impl Fn(f64) + 'static) {
        *self.ivars().on_commit.borrow_mut() = Some(Box::new(f));
    }

    pub fn value(&self) -> f64 {
        self.ivars().value.get()
    }

    pub fn is_tracking(&self) -> bool {
        self.ivars().tracking.get()
    }

    /// Set the value programmatically (ignored while the user drags).
    pub fn set_value(&self, v: f64) {
        if self.is_tracking() {
            return;
        }
        self.ivars().value.set(v.clamp(0.0, 1.0));
        self.layout_layers();
    }

    pub fn set_fill_color(&self, c: &NSColor) {
        self.ivars().fill.setBackgroundColor(Some(&c.CGColor()));
    }

    fn inset(&self) -> f64 {
        if self.ivars().knob { 7.0 } else { 0.0 }
    }

    fn layout_layers(&self) {
        let size = self.frame().size;
        let iv = self.ivars();
        let pad = self.inset();
        let usable = (size.width - 2.0 * pad).max(1.0);
        let y = (size.height - 4.0) / 2.0;
        let x = pad + usable * iv.value.get();
        without_animation(|| {
            iv.track.setFrame(rect(pad, y, usable, 4.0));
            iv.fill.setFrame(rect(pad, y, (x - pad).max(0.0), 4.0));
            iv.knob_layer.setFrame(rect(x - 7.0, size.height / 2.0 - 7.0, 14.0, 14.0));
        });
        // SAFETY: accessibility value setter on NSView.
        let pct = NSString::from_str(&format!("{:.0}%", iv.value.get() * 100.0));
        let _: () = unsafe { msg_send![self, setAccessibilityValue: &*pct] };
    }

    fn track_event(&self, event: &objc2_app_kit::NSEvent, commit: bool) {
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        let pad = self.inset();
        let usable = (self.frame().size.width - 2.0 * pad).max(1.0);
        let v = ((p.x - pad) / usable).clamp(0.0, 1.0);
        self.ivars().value.set(v);
        self.layout_layers();
        if let Some(f) = self.ivars().on_change.borrow().as_ref() {
            f(v);
        }
        if commit {
            if let Some(f) = self.ivars().on_commit.borrow().as_ref() {
                f(v);
            }
        }
    }
}

/// Several styled runs in one string ("Spotify · Playing on AirPods Pro").
pub fn attributed_parts(parts: &[(&str, &NSFont, &NSColor)]) -> Retained<objc2_foundation::NSAttributedString> {
    let m = objc2_foundation::NSMutableAttributedString::new();
    for (t, f, c) in parts {
        m.appendAttributedString(&attributed(t, f, c));
    }
    Retained::into_super(m)
}

/// Width the attributed string needs on one line.
pub fn text_width(s: &objc2_foundation::NSAttributedString) -> f64 {
    use objc2_app_kit::NSAttributedStringNSStringDrawing;
    s.size().width.ceil()
}

/// A label showing an attributed string.
pub fn rich_label(
    mtm: MainThreadMarker,
    s: &objc2_foundation::NSAttributedString,
    frame: NSRect,
) -> Retained<NSTextField> {
    let _ = mtm;
    // SAFETY: NSTextField's attributed label factory, sent to our subclass.
    let l: Retained<NSTextField> = unsafe { msg_send![text_label_class(), labelWithAttributedString: s] };
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    l.setFrame(text_frame(frame));
    l
}

/// Plain layer-backed rounded rectangle (dots, tracks, chips).
pub fn shape_view(mtm: MainThreadMarker, frame: NSRect, color: &NSColor, radius: f64) -> Retained<FlippedView> {
    let v = FlippedView::new(mtm, frame);
    make_card(&v, color, radius);
    v
}
