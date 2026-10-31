//! The notch UI state machine.
//!
//! Pure and deterministic: [`NotchMachine::handle`] consumes an [`Event`] and
//! returns the [`Effect`]s the platform layer must perform (change layout,
//! (re)arm or cancel timers, toggle mouse interactivity, take keyboard focus).
//! Timers are owned by the platform layer and come back as
//! [`Event::TimerFired`], which keeps this module free of clocks and fully
//! testable.
//!
//! ```text
//!             hover-intent                       click
//! Collapsed ───────────────▶ Hover ─────────────────────────▶ Expanded (pinned)
//!   ▲  │ notification          │ leave + grace                  │ Esc / click outside
//!   │  ▼                       ▼                                ▼
//!   └─ Peek (timeout) ◀── notification while collapsed       Collapsed
//!   drag-enter ─▶ DropTarget ─ drag-exit + grace ─▶ previous state
//! ```

use crate::geometry::LayoutKind;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Collapsed,
    /// Pointer entered the activation zone; waiting for hover intent.
    Arming,
    Peek,
    Hover,
    Expanded,
    /// Files are being dragged over the notch. `return_to` is where we go back
    /// if the drag leaves without dropping.
    DropTarget {
        return_to: ReturnTo,
    },
}

/// Subset of states a drop target can return to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReturnTo {
    Collapsed,
    Hover,
    Expanded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimerKind {
    /// Pointer has rested in the activation zone long enough.
    HoverIntent,
    /// Grace period after the pointer left before collapsing.
    CollapseGrace,
    /// Peek banner display time.
    PeekTimeout,
    /// Grace period after a drag left the zone.
    DragExitGrace,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    PointerEnteredZone,
    PointerLeftZone,
    Click,
    ClickOutside,
    Escape,
    /// A notification that qualifies for a peek arrived.
    NotificationArrived,
    DragEntered,
    DragExited,
    Dropped,
    TimerFired(TimerKind),
    /// Menu bar / hotkey toggle.
    Toggle,
    /// Force collapse (screen change, sleep, settings opened).
    Reset,
    /// Live indicators appeared or disappeared (affects collapsed layout).
    WingsChanged(bool),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Effect {
    SetLayout(LayoutKind),
    Schedule(TimerKind, Duration),
    Cancel(TimerKind),
    /// `false` = window ignores mouse events (click-through while collapsed).
    SetInteractive(bool),
    /// Become key window (for Esc / keyboard navigation) or resign.
    SetKey(bool),
    /// Mark notifications as seen (they were displayed in full).
    MarkSeen,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timings {
    pub hover_intent: Duration,
    pub collapse_grace: Duration,
    pub peek: Duration,
    pub drag_exit_grace: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            hover_intent: Duration::from_millis(120),
            collapse_grace: Duration::from_millis(350),
            peek: Duration::from_millis(4000),
            drag_exit_grace: Duration::from_millis(600),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NotchMachine {
    state: State,
    timings: Timings,
    wings: bool,
}

impl NotchMachine {
    pub fn new(timings: Timings) -> Self {
        Self { state: State::Collapsed, timings, wings: false }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn set_timings(&mut self, timings: Timings) {
        self.timings = timings;
    }

    /// The layout that corresponds to the current state.
    pub fn layout(&self) -> LayoutKind {
        match self.state {
            State::Collapsed | State::Arming => self.collapsed_layout(),
            State::Peek => LayoutKind::Peek,
            State::Hover => LayoutKind::Compact,
            State::Expanded | State::DropTarget { .. } => LayoutKind::Expanded,
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state, State::Hover | State::Expanded | State::DropTarget { .. })
    }

    fn collapsed_layout(&self) -> LayoutKind {
        if self.wings { LayoutKind::CollapsedWithWings } else { LayoutKind::Collapsed }
    }

    fn go(&mut self, to: State, fx: &mut Vec<Effect>) {
        if to == self.state {
            return;
        }
        let was_open = self.is_open();
        let before = self.layout();
        self.state = to;
        let after = self.layout();
        if before != after {
            fx.push(Effect::SetLayout(after));
        }
        let interactive = !matches!(to, State::Collapsed | State::Arming);
        fx.push(Effect::SetInteractive(interactive));
        let wants_key = matches!(to, State::Expanded);
        fx.push(Effect::SetKey(wants_key));
        if !was_open && matches!(to, State::Expanded | State::Hover) {
            fx.push(Effect::MarkSeen);
        }
    }

    fn collapse(&mut self, fx: &mut Vec<Effect>) {
        for t in [TimerKind::HoverIntent, TimerKind::CollapseGrace, TimerKind::PeekTimeout, TimerKind::DragExitGrace] {
            fx.push(Effect::Cancel(t));
        }
        self.go(State::Collapsed, fx);
    }

    pub fn handle(&mut self, event: Event) -> Vec<Effect> {
        use Event as E;
        use State as S;
        let mut fx = Vec::new();
        let t = self.timings;
        match (self.state, event) {
            // ── global events ────────────────────────────────────────────
            (_, E::Reset) => self.collapse(&mut fx),
            (_, E::WingsChanged(on)) => {
                let before = self.layout();
                self.wings = on;
                let after = self.layout();
                if before != after {
                    fx.push(Effect::SetLayout(after));
                }
            }

            // ── collapsed ────────────────────────────────────────────────
            (S::Collapsed, E::PointerEnteredZone) => {
                if t.hover_intent.is_zero() {
                    self.go(S::Hover, &mut fx);
                } else {
                    self.state = S::Arming;
                    fx.push(Effect::Schedule(TimerKind::HoverIntent, t.hover_intent));
                }
            }
            (S::Collapsed | S::Arming, E::Click | E::Toggle) => {
                fx.push(Effect::Cancel(TimerKind::HoverIntent));
                self.go(S::Expanded, &mut fx);
            }
            (S::Collapsed | S::Arming, E::NotificationArrived) => {
                fx.push(Effect::Cancel(TimerKind::HoverIntent));
                self.go(S::Peek, &mut fx);
                fx.push(Effect::Schedule(TimerKind::PeekTimeout, t.peek));
            }
            (S::Arming, E::TimerFired(TimerKind::HoverIntent)) => self.go(S::Hover, &mut fx),
            (S::Arming, E::PointerLeftZone) => {
                fx.push(Effect::Cancel(TimerKind::HoverIntent));
                self.state = S::Collapsed;
            }

            // ── peek ─────────────────────────────────────────────────────
            (S::Peek, E::TimerFired(TimerKind::PeekTimeout)) => self.go(S::Collapsed, &mut fx),
            (S::Peek, E::NotificationArrived) => {
                // Extend the banner rather than flapping.
                fx.push(Effect::Schedule(TimerKind::PeekTimeout, t.peek));
            }
            (S::Peek, E::PointerEnteredZone) => {
                fx.push(Effect::Cancel(TimerKind::PeekTimeout));
                self.go(S::Hover, &mut fx);
            }
            (S::Peek, E::Click | E::Toggle) => {
                fx.push(Effect::Cancel(TimerKind::PeekTimeout));
                self.go(S::Expanded, &mut fx);
            }
            (S::Peek, E::Escape | E::ClickOutside) => self.collapse(&mut fx),

            // ── hover ────────────────────────────────────────────────────
            (S::Hover, E::PointerLeftZone) => {
                fx.push(Effect::Schedule(TimerKind::CollapseGrace, t.collapse_grace));
            }
            (S::Hover, E::PointerEnteredZone) => fx.push(Effect::Cancel(TimerKind::CollapseGrace)),
            (S::Hover, E::TimerFired(TimerKind::CollapseGrace)) => self.go(S::Collapsed, &mut fx),
            (S::Hover, E::Click) => {
                fx.push(Effect::Cancel(TimerKind::CollapseGrace));
                self.go(S::Expanded, &mut fx);
            }
            (S::Hover, E::Escape | E::ClickOutside | E::Toggle) => self.collapse(&mut fx),

            // ── expanded (pinned) ────────────────────────────────────────
            (S::Expanded, E::Escape | E::ClickOutside | E::Toggle) => self.collapse(&mut fx),

            // ── drag & drop ──────────────────────────────────────────────
            (S::Collapsed | S::Arming | S::Peek | S::Hover | S::Expanded, E::DragEntered) => {
                let return_to = match self.state {
                    S::Hover => ReturnTo::Hover,
                    S::Expanded => ReturnTo::Expanded,
                    _ => ReturnTo::Collapsed,
                };
                for k in [TimerKind::HoverIntent, TimerKind::CollapseGrace, TimerKind::PeekTimeout] {
                    fx.push(Effect::Cancel(k));
                }
                self.go(S::DropTarget { return_to }, &mut fx);
            }
            (S::DropTarget { .. }, E::DragEntered) => fx.push(Effect::Cancel(TimerKind::DragExitGrace)),
            (S::DropTarget { .. }, E::DragExited) => {
                fx.push(Effect::Schedule(TimerKind::DragExitGrace, t.drag_exit_grace));
            }
            (S::DropTarget { return_to }, E::TimerFired(TimerKind::DragExitGrace)) => {
                let to = match return_to {
                    ReturnTo::Collapsed => S::Collapsed,
                    ReturnTo::Hover => S::Hover,
                    ReturnTo::Expanded => S::Expanded,
                };
                self.go(to, &mut fx);
            }
            // After a drop, stay open so the user sees the new tray item.
            (S::DropTarget { .. }, E::Dropped) => {
                fx.push(Effect::Cancel(TimerKind::DragExitGrace));
                self.go(S::Expanded, &mut fx);
            }
            (S::DropTarget { .. }, E::Escape) => self.collapse(&mut fx),

            // Everything else (stale timers, redundant pointer events) is ignored.
            _ => {}
        }
        fx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layouts(fx: &[Effect]) -> Vec<LayoutKind> {
        fx.iter().filter_map(|e| if let Effect::SetLayout(l) = e { Some(*l) } else { None }).collect()
    }

    fn m() -> NotchMachine {
        NotchMachine::new(Timings::default())
    }

    #[test]
    fn hover_requires_intent() {
        let mut m = m();
        let fx = m.handle(Event::PointerEnteredZone);
        assert_eq!(m.state(), State::Arming);
        assert!(fx.contains(&Effect::Schedule(TimerKind::HoverIntent, Duration::from_millis(120))));
        assert!(layouts(&fx).is_empty());
        let fx = m.handle(Event::TimerFired(TimerKind::HoverIntent));
        assert_eq!(m.state(), State::Hover);
        assert_eq!(layouts(&fx), vec![LayoutKind::Compact]);
        assert!(fx.contains(&Effect::SetInteractive(true)));
        assert!(fx.contains(&Effect::MarkSeen));
    }

    #[test]
    fn brushing_past_does_not_open() {
        let mut m = m();
        m.handle(Event::PointerEnteredZone);
        let fx = m.handle(Event::PointerLeftZone);
        assert_eq!(m.state(), State::Collapsed);
        assert!(fx.contains(&Effect::Cancel(TimerKind::HoverIntent)));
        // A stale timer must not open it.
        let fx = m.handle(Event::TimerFired(TimerKind::HoverIntent));
        assert!(fx.is_empty());
        assert_eq!(m.state(), State::Collapsed);
    }

    #[test]
    fn hover_collapses_after_grace_unless_pointer_returns() {
        let mut m = m();
        m.handle(Event::PointerEnteredZone);
        m.handle(Event::TimerFired(TimerKind::HoverIntent));
        m.handle(Event::PointerLeftZone);
        let fx = m.handle(Event::PointerEnteredZone);
        assert!(fx.contains(&Effect::Cancel(TimerKind::CollapseGrace)));
        m.handle(Event::PointerLeftZone);
        let fx = m.handle(Event::TimerFired(TimerKind::CollapseGrace));
        assert_eq!(m.state(), State::Collapsed);
        assert_eq!(layouts(&fx), vec![LayoutKind::Collapsed]);
        assert!(fx.contains(&Effect::SetInteractive(false)));
    }

    #[test]
    fn click_pins_and_escape_or_outside_click_collapses() {
        let mut m = m();
        let fx = m.handle(Event::Click);
        assert_eq!(m.state(), State::Expanded);
        assert!(fx.contains(&Effect::SetKey(true)));
        // Leaving with the pointer does not collapse a pinned panel.
        assert!(m.handle(Event::PointerLeftZone).is_empty());
        m.handle(Event::Escape);
        assert_eq!(m.state(), State::Collapsed);
        m.handle(Event::Toggle);
        m.handle(Event::ClickOutside);
        assert_eq!(m.state(), State::Collapsed);
    }

    #[test]
    fn peek_times_out_and_is_extended_by_bursts() {
        let mut m = m();
        let fx = m.handle(Event::NotificationArrived);
        assert_eq!(layouts(&fx), vec![LayoutKind::Peek]);
        let fx = m.handle(Event::NotificationArrived);
        assert_eq!(fx, vec![Effect::Schedule(TimerKind::PeekTimeout, Duration::from_millis(4000))]);
        m.handle(Event::TimerFired(TimerKind::PeekTimeout));
        assert_eq!(m.state(), State::Collapsed);
    }

    #[test]
    fn notifications_do_not_disturb_open_panel() {
        let mut m = m();
        m.handle(Event::Click);
        assert!(m.handle(Event::NotificationArrived).is_empty());
        assert_eq!(m.state(), State::Expanded);
    }

    #[test]
    fn drag_returns_to_previous_state() {
        let mut m = m();
        m.handle(Event::DragEntered);
        assert_eq!(m.state(), State::DropTarget { return_to: ReturnTo::Collapsed });
        assert_eq!(m.layout(), LayoutKind::Expanded);
        m.handle(Event::DragExited);
        // Re-entering cancels the grace timer.
        let fx = m.handle(Event::DragEntered);
        assert!(fx.contains(&Effect::Cancel(TimerKind::DragExitGrace)));
        m.handle(Event::DragExited);
        m.handle(Event::TimerFired(TimerKind::DragExitGrace));
        assert_eq!(m.state(), State::Collapsed);
    }

    #[test]
    fn drop_pins_panel() {
        let mut m = m();
        m.handle(Event::DragEntered);
        m.handle(Event::Dropped);
        assert_eq!(m.state(), State::Expanded);
    }

    #[test]
    fn wings_change_collapsed_layout_only() {
        let mut m = m();
        let fx = m.handle(Event::WingsChanged(true));
        assert_eq!(layouts(&fx), vec![LayoutKind::CollapsedWithWings]);
        m.handle(Event::Click);
        let fx = m.handle(Event::WingsChanged(false));
        assert!(layouts(&fx).is_empty());
        let fx = m.handle(Event::Escape);
        assert_eq!(layouts(&fx), vec![LayoutKind::Collapsed]);
    }

    #[test]
    fn zero_hover_delay_opens_immediately() {
        let mut m = NotchMachine::new(Timings { hover_intent: Duration::ZERO, ..Timings::default() });
        m.handle(Event::PointerEnteredZone);
        assert_eq!(m.state(), State::Hover);
    }
}
