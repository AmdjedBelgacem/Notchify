//! Damped-spring animation.
//!
//! Springs are integrated with semi-implicit (symplectic) Euler at a fixed
//! sub-step, which is stable for the stiffness range we use and makes the
//! motion independent of the display refresh rate (60 Hz vs. 120 Hz ProMotion).
//!
//! The animator in the macOS layer only runs its timer while
//! [`SpringRect::is_settled`] is `false`, so a resting UI costs no CPU.

/// Physical parameters. Use [`SpringParams::with_response`] to think in terms
/// of "response time" and "damping fraction" like SwiftUI's `.spring()`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringParams {
    pub stiffness: f64,
    pub damping: f64,
    pub mass: f64,
}

impl SpringParams {
    /// `response`: period of the undamped oscillation in seconds.
    /// `damping_fraction`: 1.0 = critically damped, < 1.0 bounces.
    pub fn with_response(response: f64, damping_fraction: f64) -> Self {
        let response = response.max(0.01);
        let stiffness = (2.0 * std::f64::consts::PI / response).powi(2);
        let damping = 4.0 * std::f64::consts::PI * damping_fraction / response;
        Self { stiffness, damping, mass: 1.0 }
    }

    /// Opening: slightly bouncy, feels alive.
    pub fn open() -> Self {
        Self::with_response(0.42, 0.78)
    }

    /// Closing: no overshoot so the island never pokes below the notch.
    pub fn close() -> Self {
        Self::with_response(0.36, 1.0)
    }

    /// Scale "speed" (stiffness) keeping the damping fraction constant.
    pub fn scaled(self, speed: f64) -> Self {
        let speed = speed.clamp(0.1, 10.0);
        Self { stiffness: self.stiffness * speed * speed, damping: self.damping * speed, mass: self.mass }
    }
}

const SUBSTEP: f64 = 1.0 / 240.0;
/// Large frame gaps (app nap, debugger) are clamped so springs never explode.
const MAX_DT: f64 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub value: f64,
    pub velocity: f64,
    pub target: f64,
    pub params: SpringParams,
    /// Settle thresholds (absolute units of `value`).
    pub epsilon: f64,
}

impl Spring {
    pub fn new(value: f64, params: SpringParams) -> Self {
        Self { value, velocity: 0.0, target: value, params, epsilon: 0.05 }
    }

    pub fn with_epsilon(mut self, epsilon: f64) -> Self {
        self.epsilon = epsilon;
        self
    }

    pub fn set_target(&mut self, target: f64, params: SpringParams) {
        self.target = target;
        self.params = params;
    }

    /// Jump straight to the target (reduced motion).
    pub fn snap(&mut self) {
        self.value = self.target;
        self.velocity = 0.0;
    }

    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < self.epsilon && self.velocity.abs() < self.epsilon * 10.0
    }

    pub fn step(&mut self, dt: f64) {
        let mut remaining = dt.clamp(0.0, MAX_DT);
        while remaining > 0.0 {
            let h = remaining.min(SUBSTEP);
            let SpringParams { stiffness, damping, mass } = self.params;
            let force = -stiffness * (self.value - self.target) - damping * self.velocity;
            self.velocity += force / mass * h;
            self.value += self.velocity * h;
            remaining -= h;
        }
        if self.is_settled() {
            self.snap();
        }
    }
}

/// The animated parameters of the island.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringRect {
    pub width: Spring,
    pub height: Spring,
    pub radius: Spring,
    /// Opacity of the content of the *current* layout (0–1).
    pub content_alpha: Spring,
}

impl SpringRect {
    pub fn new(width: f64, height: f64, radius: f64) -> Self {
        let p = SpringParams::close();
        Self {
            width: Spring::new(width, p),
            height: Spring::new(height, p),
            radius: Spring::new(radius, p),
            content_alpha: Spring::new(1.0, p).with_epsilon(0.004),
        }
    }

    /// Animate towards a new shape. Growing uses the bouncy spring, shrinking
    /// the critically damped one.
    pub fn animate_to(&mut self, width: f64, height: f64, radius: f64, speed: f64) {
        let growing = width * height >= self.width.target * self.height.target;
        let p = if growing { SpringParams::open() } else { SpringParams::close() }.scaled(speed);
        self.width.set_target(width, p);
        self.height.set_target(height, p);
        self.radius.set_target(radius, SpringParams::close().scaled(speed));
    }

    /// Fade content out then (via the caller) back in when the layout changes.
    pub fn set_content_alpha(&mut self, alpha: f64, speed: f64) {
        self.content_alpha.set_target(alpha, SpringParams::with_response(0.22, 1.0).scaled(speed));
    }

    pub fn step(&mut self, dt: f64) {
        self.width.step(dt);
        self.height.step(dt);
        self.radius.step(dt);
        self.content_alpha.step(dt);
    }

    pub fn snap(&mut self) {
        self.width.snap();
        self.height.snap();
        self.radius.snap();
        self.content_alpha.snap();
    }

    pub fn is_settled(&self) -> bool {
        self.width.is_settled() && self.height.is_settled() && self.radius.is_settled() && self.content_alpha.is_settled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(spring: &mut Spring, seconds: f64, fps: f64) -> f64 {
        let mut max = spring.value;
        let frames = (seconds * fps) as usize;
        for _ in 0..frames {
            spring.step(1.0 / fps);
            max = max.max(spring.value);
        }
        max
    }

    #[test]
    fn critically_damped_spring_does_not_overshoot() {
        let mut s = Spring::new(0.0, SpringParams::close());
        s.set_target(100.0, SpringParams::close());
        let max = run(&mut s, 2.0, 60.0);
        assert!(max <= 100.0 + 1e-6, "overshoot: {max}");
        assert!(s.is_settled());
        assert_eq!(s.value, 100.0);
    }

    #[test]
    fn open_spring_overshoots_slightly_and_settles() {
        let mut s = Spring::new(0.0, SpringParams::open());
        s.set_target(100.0, SpringParams::open());
        let max = run(&mut s, 2.0, 120.0);
        assert!(max > 100.0 && max < 106.0, "max {max}");
        assert!(s.is_settled());
    }

    #[test]
    fn frame_rate_independent() {
        let mut a = Spring::new(0.0, SpringParams::open());
        let mut b = a;
        a.set_target(50.0, SpringParams::open());
        b.set_target(50.0, SpringParams::open());
        run(&mut a, 0.2, 60.0);
        run(&mut b, 0.2, 120.0);
        assert!((a.value - b.value).abs() < 0.5, "{} vs {}", a.value, b.value);
    }

    #[test]
    fn huge_dt_is_clamped() {
        let mut s = Spring::new(0.0, SpringParams::open());
        s.set_target(10.0, SpringParams::open());
        s.step(30.0);
        assert!(s.value.is_finite() && s.value.abs() < 100.0);
    }

    #[test]
    fn rect_settles_and_snaps() {
        let mut r = SpringRect::new(185.0, 32.0, 10.0);
        assert!(r.is_settled());
        r.animate_to(680.0, 352.0, 30.0, 1.0);
        assert!(!r.is_settled());
        for _ in 0..240 {
            r.step(1.0 / 120.0);
        }
        assert!(r.is_settled());
        assert_eq!(r.width.value, 680.0);
        r.animate_to(185.0, 32.0, 10.0, 1.0);
        r.snap();
        assert_eq!(r.height.value, 32.0);
    }

    #[test]
    fn faster_speed_settles_sooner() {
        let settle = |speed: f64| {
            let mut s = Spring::new(0.0, SpringParams::close());
            s.set_target(100.0, SpringParams::close().scaled(speed));
            let mut t = 0;
            while !s.is_settled() && t < 10_000 {
                s.step(1.0 / 120.0);
                t += 1;
            }
            t
        };
        assert!(settle(2.0) < settle(1.0));
    }
}
