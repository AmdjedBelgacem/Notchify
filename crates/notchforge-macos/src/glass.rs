//! The island's material: pure black at the notch, Liquid Glass at the ends,
//! built to the *Glass · material & motion* board.
//!
//! From the back: the glass itself, a black veil that thins out away from the
//! notch, a solid black band behind the notch row, a lit lower edge and rim,
//! and a solid black layer that clears as the island opens. A collapsed island
//! is only that black layer, so it still merges with the bezel.

use crate::ui::{FlippedView, rect, rgba, set_gradient, without_animation};
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSGlassEffectView, NSGlassEffectViewStyle, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::{NSArray, NSNumber, NSPoint};
use objc2_quartz_core::{CAGradientLayer, CALayer};

/// How far the black reaches from the notch, as a fraction of the veil's radii.
const REACH: f64 = 0.40;
/// Black left at the ends of the open panel, where the glass is clearest.
const TINT_OPEN: f64 = 0.26;
/// Hover and peek have free text on the material, so they keep more black.
const TINT_SMALL: f64 = 0.52;
const TINT_HIGH_CONTRAST: f64 = 0.70;
/// Height of the fade below the notch row.
const BAND_FADE: f64 = 30.0;
const LIP: f64 = 18.0;

pub struct Glass {
    effect: Retained<NSView>,
    glass: Option<Retained<NSGlassEffectView>>,
    host: Retained<FlippedView>,
    veil: Retained<CAGradientLayer>,
    band: Retained<CAGradientLayer>,
    lip: Retained<CAGradientLayer>,
    rim: Retained<CALayer>,
    rim_mask: Retained<CAGradientLayer>,
    black: Retained<CALayer>,
}

fn locations(values: &[f64]) -> Retained<NSArray<NSNumber>> {
    let numbers: Vec<Retained<NSNumber>> = values.iter().map(|v| NSNumber::new_f64(*v)).collect();
    NSArray::from_retained_slice(&numbers)
}

/// The system's Liquid Glass where it exists, else its blur material.
fn effect_view(mtm: MainThreadMarker) -> (Retained<NSView>, Option<Retained<NSGlassEffectView>>) {
    if AnyClass::get(c"NSGlassEffectView").is_some() {
        let g = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), rect(0.0, 0.0, 1.0, 1.0));
        g.setStyle(NSGlassEffectViewStyle::Clear);
        return (Retained::into_super(g.clone()), Some(g));
    }
    let v = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), rect(0.0, 0.0, 1.0, 1.0));
    v.setMaterial(NSVisualEffectMaterial::HUDWindow);
    v.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    v.setState(NSVisualEffectState::Active);
    (Retained::into_super(v), None)
}

fn build_host(mtm: MainThreadMarker, nh: f64) -> Glass {
    let (effect, glass) = effect_view(mtm);
    let host = FlippedView::new(mtm, rect(0.0, 0.0, 1.0, 1.0));
    host.setWantsLayer(true);

    let veil = CAGradientLayer::layer();
    // SAFETY: "radial" is kCAGradientLayerRadial.
    veil.setType(&objc2_foundation::NSString::from_str("radial"));
    veil.setStartPoint(NSPoint::new(0.5, 0.0));
    veil.setEndPoint(NSPoint::new(0.5 + 1.15, 1.3));
    veil.setLocations(Some(&locations(&[0.0, REACH, REACH + 0.22, 1.0])));

    let band = CAGradientLayer::layer();
    band.setStartPoint(NSPoint::new(0.5, 0.0));
    band.setEndPoint(NSPoint::new(0.5, 1.0));
    set_gradient(&band, &[(0.0, 0.0, 0.0, 1.0), (0.0, 0.0, 0.0, 1.0), (0.0, 0.0, 0.0, 0.0)]);

    let lip = CAGradientLayer::layer();
    lip.setStartPoint(NSPoint::new(0.5, 0.0));
    lip.setEndPoint(NSPoint::new(0.5, 1.0));
    set_gradient(&lip, &[(1.0, 1.0, 1.0, 0.0), (1.0, 1.0, 1.0, 0.14)]);

    let rim = CALayer::new();
    rim.setBorderWidth(0.5);
    rim.setBorderColor(Some(&rgba(1.0, 1.0, 1.0, 0.5).CGColor()));
    let rim_mask = CAGradientLayer::layer();
    rim_mask.setStartPoint(NSPoint::new(0.5, 0.0));
    rim_mask.setEndPoint(NSPoint::new(0.5, 1.0));
    set_gradient(&rim_mask, &[(0.0, 0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 0.0), (0.0, 0.0, 0.0, 1.0)]);
    rim_mask.setLocations(Some(&locations(&[0.0, 0.34, 0.82])));
    // SAFETY: a plain gradient layer used as a mask.
    unsafe { rim.setMask(Some(&rim_mask)) };

    let black = CALayer::new();
    black.setBackgroundColor(Some(&objc2_app_kit::NSColor::blackColor().CGColor()));

    if let Some(l) = host.layer() {
        l.addSublayer(&veil);
        l.addSublayer(&band);
        l.addSublayer(&lip);
        l.addSublayer(&rim);
        l.addSublayer(&black);
    }
    let this = Glass { effect, glass, host, veil, band, lip, rim, rim_mask, black };
    this.set_notch_height(nh);
    this.set_tint(TINT_OPEN);
    this
}

impl Glass {
    /// Add the material to `island`, behind anything added later.
    pub fn install(mtm: MainThreadMarker, island: &NSView, nh: f64) -> Self {
        let this = build_host(mtm, nh);
        island.addSubview(&this.effect);
        island.addSubview(&this.host);
        this
    }

    /// The material over a stand-in desktop, for self-test renders: the blur
    /// happens in the window server, so it cannot be captured offscreen.
    pub fn preview(mtm: MainThreadMarker, island: &NSView, w: f64, h: f64, r: f64, nh: f64, t: f64) -> Self {
        let this = build_host(mtm, nh);
        if let Some(l) = island.layer() {
            let desk = crate::ui::gradient_layer(
                &[(0.42, 0.26, 0.62, 1.0), (0.84, 0.30, 0.42, 1.0), (0.94, 0.58, 0.30, 1.0)],
                NSPoint::new(0.0, 1.0),
                NSPoint::new(1.0, 0.0),
            );
            desk.setFrame(rect(0.0, 0.0, w, h + r));
            l.addSublayer(&desk);
        }
        island.addSubview(&this.host);
        this.apply(w, h, r, r, t);
        this
    }

    /// Set the fade below the notch row.
    pub fn set_notch_height(&self, nh: f64) {
        let total = nh + BAND_FADE;
        self.band.setLocations(Some(&locations(&[0.0, nh / total, 1.0])));
        without_animation(|| self.band.setBounds(rect(0.0, 0.0, 1.0, total)));
    }

    fn set_tint(&self, tint: f64) {
        set_gradient(
            &self.veil,
            &[
                (0.0, 0.0, 0.0, 1.0),
                (0.0, 0.0, 0.0, 1.0),
                (0.0, 0.0, 0.0, (tint + 0.36).min(1.0)),
                (0.0, 0.0, 0.0, tint),
            ],
        );
    }

    /// Choose how much black stays at the ends for the shape being shown.
    pub fn set_open_panel(&self, open_panel: bool, high_contrast: bool) {
        let tint = if high_contrast {
            TINT_HIGH_CONTRAST
        } else if open_panel {
            TINT_OPEN
        } else {
            TINT_SMALL
        };
        without_animation(|| self.set_tint(tint));
    }

    /// Lay out for an island `w` × `h` with `overhang` pt above the window, at
    /// `t` from solid black (0) to settled glass (1).
    pub fn apply(&self, w: f64, h: f64, overhang: f64, radius: f64, t: f64) {
        let t = if system_reduce_transparency() { 0.0 } else { t.clamp(0.0, 1.0) };
        self.effect.setHidden(t <= 0.001);
        self.effect.setFrame(rect(0.0, 0.0, w, h + overhang));
        if let Some(g) = &self.glass {
            g.setCornerRadius(radius);
        }
        self.host.setFrame(rect(0.0, 0.0, w, h));
        without_animation(|| {
            self.veil.setFrame(rect(0.0, 0.0, w, h));
            let band_h = self.band.bounds().size.height;
            self.band.setFrame(rect(0.0, 0.0, w, band_h));
            self.lip.setFrame(rect(0.0, h - LIP, w, LIP));
            self.rim.setFrame(rect(0.0, -overhang, w, h + overhang));
            self.rim.setCornerRadius(radius);
            self.rim_mask.setFrame(rect(0.0, 0.0, w, h + overhang));
            self.black.setFrame(rect(0.0, 0.0, w, h));
            self.black.setOpacity((1.0 - t) as f32);
            self.rim.setOpacity(t as f32);
            self.lip.setOpacity(t as f32);
        });
    }
}

pub fn system_reduce_transparency() -> bool {
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceTransparency()
}
