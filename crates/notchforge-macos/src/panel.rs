//! The notch window and its custom views.
//!
//! * [`NotchPanel`] — borderless, non-activating `NSPanel` above the menu bar
//!   on every Space (including full-screen apps). Overrides
//!   `canBecomeKeyWindow` so a pinned panel can receive Escape.
//! * [`RootView`] — transparent content view: forwards pointer, click, key and
//!   drag-and-drop events to the [`Controller`](crate::controller::Controller).
//! * [`TrayItemView`] — one file in the tray; a drag *source* so files can be
//!   dragged out to Finder or any other app.

use crate::app::with_controller;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSDragOperation, NSDraggingContext, NSDraggingDestination, NSDraggingInfo,
    NSDraggingItem, NSDraggingSession, NSDraggingSource, NSEvent, NSMainMenuWindowLevel, NSPanel,
    NSPasteboardTypeFileURL, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSURL};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;

// ─────────────────────────────────────────────────────────────────────────────
// NotchPanel
// ─────────────────────────────────────────────────────────────────────────────

define_class!(
    // SAFETY: NSPanel subclass without ivars or Drop.
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    pub struct NotchPanel;

    impl NotchPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            false
        }
    }
);

impl NotchPanel {
    pub fn create(mtm: MainThreadMarker, frame: NSRect, content: &NSView) -> Retained<Self> {
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        let this = Self::alloc(mtm);
        // SAFETY: NSWindow's designated initializer.
        let panel: Retained<Self> = unsafe {
            msg_send![this, initWithContentRect: frame, styleMask: style, backing: NSBackingStoreType::Buffered, defer: false]
        };
        // Above the menu bar (24) so the island covers it, below screen savers.
        panel.setLevel(NSMainMenuWindowLevel + 3);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(false);
        panel.setIgnoresMouseEvents(true);
        panel.setAcceptsMouseMovedEvents(true);
        panel.setMovable(false);
        panel.setHidesOnDeactivate(false);
        panel.setFloatingPanel(true);
        // SAFETY: we keep our own strong reference; AppKit must not release on close.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setContentView(Some(content));
        panel.makeFirstResponder(Some(content));
        panel
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RootView
// ─────────────────────────────────────────────────────────────────────────────

define_class!(
    // SAFETY: NSView subclass without ivars or Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    pub struct RootView;

    impl RootView {
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let handled = with_controller(|c| c.key_down(event.keyCode(), event.modifierFlags())).unwrap_or(false);
            if !handled {
                // SAFETY: forwarding to NSView's implementation.
                let _: () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let p = event.locationInWindow();
            with_controller(|c| c.root_mouse_down(p));
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, _event: &NSEvent) {
            with_controller(|c| c.pointer_moved(NSEvent::mouseLocation()));
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            with_controller(|c| c.pointer_moved(NSEvent::mouseLocation()));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            with_controller(|c| c.pointer_moved(NSEvent::mouseLocation()));
        }
    }

    unsafe impl NSDraggingDestination for RootView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            if dragged_paths(sender).is_empty() {
                return NSDragOperation::None;
            }
            with_controller(|c| c.drag_entered());
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            if dragged_paths(sender).is_empty() { NSDragOperation::None } else { NSDragOperation::Copy }
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, _sender: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            with_controller(|c| c.drag_exited());
        }

        #[unsafe(method(prepareForDragOperation:))]
        fn prepare_for_drag(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            true
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let paths = dragged_paths(sender);
            let accepted = !paths.is_empty();
            if accepted {
                with_controller(|c| c.files_dropped(paths));
            }
            accepted
        }
    }

    unsafe impl NSObjectProtocol for RootView {}
);

/// File paths on a drag pasteboard.
fn dragged_paths(sender: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<PathBuf> {
    let pb = sender.draggingPasteboard();
    pasteboard_file_paths(&pb)
}

pub fn pasteboard_file_paths(pb: &objc2_app_kit::NSPasteboard) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(items) = pb.pasteboardItems() else { return out };
    for item in items.iter() {
        // SAFETY: reading an AppKit constant.
        let Some(s) = item.stringForType(unsafe { NSPasteboardTypeFileURL }) else { continue };
        if let Some(url) = NSURL::URLWithString(&s) {
            // `file:///.file/id=…` reference URLs resolve to real paths here.
            let url = url.filePathURL().unwrap_or(url);
            if let Some(p) = url.path() {
                out.push(PathBuf::from(p.to_string()));
            }
        }
    }
    out
}

impl RootView {
    pub fn create(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm);
        // SAFETY: NSView's designated initializer.
        let view: Retained<Self> = unsafe { msg_send![this, initWithFrame: frame] };
        // SAFETY: reading an AppKit constant.
        let types = NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]);
        view.registerForDraggedTypes(&types);
        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::ActiveAlways
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: owner is the view itself, which outlives the tracking area.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                view.bounds(),
                options,
                Some(&view),
                None,
            )
        };
        view.addTrackingArea(&area);
        view
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// TrayItemView
// ─────────────────────────────────────────────────────────────────────────────

pub struct TrayItemIvars {
    pub path: PathBuf,
    mouse_down: RefCell<Option<Retained<NSEvent>>>,
    dragging: Cell<bool>,
}

define_class!(
    // SAFETY: NSView subclass; ivars dropped by objc2.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TrayItemIvars]
    pub struct TrayItemView;

    impl TrayItemView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if event.clickCount() >= 2 {
                crate::sharing::open_path(&self.ivars().path);
                return;
            }
            *self.ivars().mouse_down.borrow_mut() = Some(event.retain());
            self.ivars().dragging.set(false);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if self.ivars().dragging.replace(true) {
                return;
            }
            let start = self.ivars().mouse_down.borrow().clone();
            self.begin_drag(start.as_deref().unwrap_or(event));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.ivars().mouse_down.borrow_mut().take();
            self.ivars().dragging.set(false);
        }
    }

    unsafe impl NSObjectProtocol for TrayItemView {}

    unsafe impl NSDraggingSource for TrayItemView {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operation_mask(&self, _session: &NSDraggingSession, _context: NSDraggingContext) -> NSDragOperation {
            // Copy is non-destructive; Finder still offers Move with ⌘.
            NSDragOperation::Copy | NSDragOperation::Generic
        }
    }
);

impl TrayItemView {
    pub fn create(mtm: MainThreadMarker, frame: NSRect, path: PathBuf) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TrayItemIvars {
            path,
            mouse_down: RefCell::new(None),
            dragging: Cell::new(false),
        });
        // SAFETY: NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn begin_drag(&self, event: &NSEvent) {
        let path = &self.ivars().path;
        let url = crate::sharing::file_url(path);
        let writer: &ProtocolObject<dyn objc2_app_kit::NSPasteboardWriting> = ProtocolObject::from_ref(&*url);
        let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), writer);
        let icon = crate::icons::file_icon(path);
        let b = self.bounds();
        let frame =
            NSRect::new(NSPoint::new((b.size.width - 40.0) / 2.0, 4.0), objc2_foundation::NSSize::new(40.0, 40.0));
        let contents: &AnyObject = &icon;
        // SAFETY: contents is an NSImage, as documented.
        unsafe { item.setDraggingFrame_contents(frame, Some(contents)) };
        let items = NSArray::from_retained_slice(&[item]);
        let source: &ProtocolObject<dyn NSDraggingSource> = ProtocolObject::from_ref(self);
        let _session = self.beginDraggingSessionWithItems_event_source(&items, event, source);
    }
}
