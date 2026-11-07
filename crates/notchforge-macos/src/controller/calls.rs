//! Ringing and ongoing calls: showing them, the call timer, answering and
//! hanging up in the calling app.

use super::*;

impl Controller {
    /// Show, resize or stop showing the ringing calls.
    pub(super) fn sync_calls(&self, snap: &Snapshot) {
        let ids: Vec<_> = snap.calls.iter().map(|c| c.id).collect();
        let is_new = ids.iter().any(|id| !self.known_calls.borrow().contains(id));
        *self.known_calls.borrow_mut() = ids;
        if is_new || (snap.calls.is_empty() && snap.active_call.is_none()) {
            self.calls_hidden.set(false);
        }
        self.set_call_timer(!snap.calls.is_empty() || snap.active_call.is_some());
        self.views.borrow().expanded.set_call(snap.active_call.as_ref(), now_ms());
        let show = !snap.calls.is_empty() && !self.calls_hidden.get();
        // Just picked up: show who with and how to hang up, then settle beside the notch.
        let just_answered = snap.active_call.as_ref().filter(|a| {
            snap.calls.is_empty() && !self.calls_hidden.get() && now_ms() - a.started_ms < JUST_ANSWERED_MS
        });
        if let Some(call) = just_answered {
            self.views.borrow().call.update_active(call, now_ms());
            self.calls_showing.set(true);
            self.update_interactive();
            if self.kind.get() != LayoutKind::Incoming(1) {
                self.set_layout(LayoutKind::Incoming(1));
            }
        } else if show {
            let shown = snap.calls.len().min(notchforge_core::geometry::MAX_VISIBLE_CALLS) as u8;
            self.views.borrow().call.update(&snap.calls, !self.reduce_motion());
            self.calls_showing.set(true);
            self.update_interactive();
            if self.kind.get() != LayoutKind::Incoming(shown) {
                self.set_layout(LayoutKind::Incoming(shown));
            }
        } else if self.calls_showing.replace(false) {
            self.update_interactive();
            self.set_layout(self.wanted_kind.get());
        }
    }

    /// Check once a second, while anything rings, for calls that stopped.
    pub(super) fn set_call_timer(&self, on: bool) {
        let mut slot = self.call_timer.borrow_mut();
        if on && slot.is_none() {
            let block = RcBlock::new(|_t: NonNull<NSTimer>| {
                crate::app::send(Command::ExpireCalls);
                crate::app::with_controller(|c| c.call_second());
            });
            // SAFETY: documented NSTimer API; the block is copied by the timer.
            let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(1.0, true, &block) };
            // SAFETY: valid timer and run loop mode constant.
            unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
            *slot = Some(timer);
        } else if !on {
            if let Some(t) = slot.take() {
                t.invalidate();
            }
        }
    }

    /// Once a second during a call: move the clocks on, and settle the view.
    pub fn call_second(&self) {
        let snap = self.snapshot.borrow().clone();
        let Some(call) = &snap.active_call else { return };
        let now = now_ms();
        self.views.borrow().call.set_clock(&notchforge_core::calls::call_clock(call.started_ms, now));
        self.render_small(&snap);
        // Rebuilding the big view each second would lose a click in progress.
        if now - call.started_ms >= JUST_ANSWERED_MS || !snap.calls.is_empty() {
            self.sync_calls(&snap);
        }
    }

    /// The process whose windows a call's buttons are in. A call seen through
    /// a notification has none yet, so its app's main process is the place to look.
    pub(super) fn call_pid(window_pid: Option<i32>, app: &notchforge_core::apps::AppRef) -> Option<i32> {
        window_pid.or_else(|| {
            let pid = crate::call_watch::main_pid(app.bundle_id.as_deref()?, 0);
            (pid != 0).then_some(pid)
        })
    }

    /// The process a call is ringing in right now. A call seen only through a
    /// notification may have no buttons yet, and the app's other "Accept" and
    /// "Decline" buttons are not this call's.
    pub(super) fn ringing_pid(call: &notchforge_core::calls::Call) -> Option<i32> {
        let pid = Self::call_pid(call.window_pid, &call.app)?;
        (call.window_pid.is_some() || crate::accessibility::inspect_call_window(pid, true).ringing).then_some(pid)
    }

    /// Press whatever ends the call: its hang-up button, or its decline button
    /// if it turns out to be ringing still.
    pub(super) fn press_hang_up(call: &notchforge_core::calls::ActiveCall) -> bool {
        use notchforge_core::calls::{DECLINE_WORDS, HANG_UP_WORDS, LEAVE_WORDS};
        Self::call_pid(call.window_pid, &call.app).is_some_and(|pid| {
            crate::accessibility::press_window_button(pid, HANG_UP_WORDS)
                || crate::accessibility::press_window_button(pid, LEAVE_WORDS)
                || (crate::accessibility::inspect_call_window(pid, true).ringing
                    && crate::accessibility::press_window_button(pid, DECLINE_WORDS))
        })
    }

    /// End the call in progress with the app's own button. If there's none to
    /// press, its tab or app may be out of view: bring it forward and try once
    /// more. Pressing hang up again means it's over, whatever the app shows.
    pub fn hang_up(&self) {
        let snap = self.snapshot.borrow().clone();
        let Some(call) = &snap.active_call else { return };
        let pressed = Self::press_hang_up(call);
        let now = now_ms();
        let again = now - self.hang_up_tried_ms.get() < HANG_UP_AGAIN_MS;
        tracing::debug!(pressed, again, source = %call.source, "call: hang up");
        let bundle = call.app.bundle_id.clone();
        match bundle {
            Some(b) if !pressed && !again => {
                self.hang_up_tried_ms.set(now);
                crate::call_watch::bring_forward(b, call.caller.clone(), call.source.clone(), None);
                let when = dispatch2::DispatchTime::NOW.time(1_500_000_000);
                let _ = dispatch2::DispatchQueue::main().after(when, || {
                    crate::app::with_controller(|c| c.hang_up_in_view());
                });
            }
            _ => {
                self.hang_up_tried_ms.set(0);
                self.engine.send(Command::EndActiveCall);
            }
        }
    }

    /// Hang up again, now the call's tab or app has come forward.
    pub(super) fn hang_up_in_view(&self) {
        let snap = self.snapshot.borrow().clone();
        let Some(call) = &snap.active_call else { return };
        if Self::press_hang_up(call) {
            tracing::debug!(source = %call.source, "call: hung up once in view");
            self.hang_up_tried_ms.set(0);
            self.engine.send(Command::EndActiveCall);
        }
    }

    /// Answer, decline or reorder a call.
    pub fn call_action(&self, call: &notchforge_core::calls::Call, action: crate::views::call::CallAction) {
        use crate::views::call::CallAction;
        use notchforge_core::calls::{ANSWER_WORDS, DECLINE_WORDS};
        match action {
            CallAction::Promote => self.engine.send(Command::PromoteCall(call.id)),
            CallAction::Answer => {
                let pressed = Self::ringing_pid(call)
                    .is_some_and(|pid| crate::accessibility::press_window_button(pid, ANSWER_WORDS))
                    || crate::accessibility::press_banner_button(&call.banner_title, ANSWER_WORDS);
                tracing::debug!(pressed, source = %call.source, "call: answer");
                if pressed {
                    self.engine.send(Command::CallAnswered(call.id));
                    return;
                }
                // Still ringing out of view: bring it forward, and the watch
                // presses its button as soon as it can be seen.
                if !crate::accessibility::open_banner(&call.banner_title) {
                    if let Some(b) = call.app.bundle_id.clone() {
                        let (caller, source) = (call.caller.clone(), call.source.clone());
                        crate::call_watch::bring_forward(b, caller, source, call.url.clone());
                    }
                }
                self.engine.send(Command::CallAnswering(call.id));
            }
            CallAction::Decline => {
                // Without a button to press, this only stops the notch ringing.
                let pressed = Self::ringing_pid(call)
                    .is_some_and(|pid| crate::accessibility::press_window_button(pid, DECLINE_WORDS))
                    || crate::accessibility::press_banner_button(&call.banner_title, DECLINE_WORDS);
                tracing::debug!(pressed, source = %call.source, "call: decline");
                self.engine.send(Command::EndCall(call.id));
            }
        }
    }

    /// Move the ringing calls out of the way until something new rings.
    pub fn hide_calls(&self) {
        self.calls_hidden.set(true);
        let snap = self.snapshot.borrow().clone();
        self.sync_calls(&snap);
    }
}
