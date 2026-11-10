//! Cancellable app-quit interception for dirty editor drafts.
//!
//! Rune's `App::on_close`/`on_quit` are notifications, not vetoes, and its
//! macOS `applicationShouldTerminate:` hook always answers `NSTerminateNow`.
//! On macOS this module swaps in its own `applicationShouldTerminate:`
//! implementation once a hook is present to wrap — Rune's own, added during
//! window creation, or one the delegate's class only inherits. A dirty draft
//! answers `NSTerminateCancel` and posts the guard to the UI; a clean quit
//! forwards to the IMP that answered before the swap, keeping its
//! `quit::start()` bookkeeping and `on_quit`/`on_close` hooks intact.
//!
//! Window-close cancellation works on every platform through `keep_running`
//! and reopen (`navigation::on_window_close`). OS-level quit (⌘Q, Dock Quit,
//! logout) can only be vetoed on macOS; elsewhere it remains an honest gap —
//! the drafts are in-memory and the process exits.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rune::AppHandle;

use crate::State;

#[derive(Clone)]
struct Slot {
    dirty: Arc<AtomicBool>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

static SLOT: OnceLock<Mutex<Option<Slot>>> = OnceLock::new();

#[cfg(test)]
thread_local! {
    /// Test seam: `install` registers into the calling thread's own slot, so
    /// a headless `EditorState::new` on a parallel test thread can never
    /// clobber another thread's native-hook fixture. The global `SLOT` still
    /// carries fixtures the tests set directly.
    static TEST_SLOT: std::cell::RefCell<Option<Slot>> =
        const { std::cell::RefCell::new(None) };
}

/// Register the dirty flag and the UI notification used when the OS vetoes a
/// quit. Called once from `EditorState::new`.
pub fn install(dirty: Arc<AtomicBool>, handle: AppHandle<State>) {
    let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let h = handle.clone();
        h.update(|s: &mut State, _| crate::editor::navigation::on_quit_vetoed(s));
    });
    write_slot(Slot { dirty, notify });
}

/// Store the slot `install` built: the calling thread's own slot under
/// `cfg(test)` so parallel test installs stay isolated…
#[cfg(test)]
fn write_slot(slot: Slot) {
    TEST_SLOT.with(|s| *s.borrow_mut() = Some(slot));
}

/// …and the process-global `SLOT` in production, where the app's single
/// `EditorState` owns it.
#[cfg(not(test))]
fn write_slot(slot: Slot) {
    *SLOT.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(slot);
}

/// What `install` leaves behind for an `EditorState::new` on the calling
/// thread — the same write minus the `AppHandle` a test cannot build.
#[cfg(test)]
fn install_test_slot(dirty: Arc<AtomicBool>, notify: Arc<dyn Fn() + Send + Sync>) {
    TEST_SLOT.with(|s| *s.borrow_mut() = Some(Slot { dirty, notify }));
}

/// The slot `should_terminate` consults under `cfg(test)`: this thread's own
/// slot when it installed one (mirroring a real install on the dispatch
/// thread), else the process-global `SLOT`.
#[cfg(test)]
fn slot() -> Option<Slot> {
    if let Some(slot) = TEST_SLOT.with(|s| s.borrow().clone()) {
        return Some(slot);
    }
    global_slot()
}

/// In production there is only the process-global `SLOT`.
#[cfg(not(test))]
fn slot() -> Option<Slot> {
    global_slot()
}

/// The process-global slot the production IMP and the test fixtures share.
fn global_slot() -> Option<Slot> {
    SLOT.get().and_then(|m| m.lock().unwrap().clone())
}

/// `AppSpec::poll` hook: (re)attempt the macOS swizzle until it lands. The
/// first poll runs after Rune installed its own hook during window creation;
/// later polls retry harmlessly if the delegate wasn't ready yet.
pub fn poll_install() {
    #[cfg(target_os = "macos")]
    if !installed() {
        macos::install();
    }
    #[cfg(not(target_os = "macos"))]
    {
        // No OS-level veto exists here; nothing to install.
    }
}

/// Whether the macOS swizzle is in place (diagnostics).
#[cfg(target_os = "macos")]
pub fn installed() -> bool {
    macos::installed()
}
#[cfg(not(target_os = "macos"))]
pub fn installed() -> bool {
    false
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::ffi::{c_char, c_void};
    use std::sync::atomic::AtomicUsize;

    use objc2::runtime::{AnyClass, AnyObject, Sel};
    use objc2::{msg_send, sel};
    use objc2_app_kit::NSApplication;
    use objc2_foundation::MainThreadMarker;

    /// Rune's `applicationShouldTerminate:` IMP, captured by the swizzle.
    static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
    static SWAPPED: AtomicBool = AtomicBool::new(false);

    /// `NSTerminateCancel`.
    const TERMINATE_CANCEL: usize = 0;
    /// `NSTerminateNow` — Rune's hook always answers this.
    const TERMINATE_NOW: usize = 1;

    unsafe extern "C" {
        fn class_getSuperclass(class: *const AnyClass) -> *const AnyClass;
        fn class_replaceMethod(
            class: *const AnyClass,
            selector: Sel,
            imp: *const c_void,
            types: *const c_char,
        ) -> *mut c_void;
    }

    extern "C" fn should_terminate(
        this: *mut AnyObject,
        cmd: Sel,
        sender: *mut AnyObject,
    ) -> usize {
        let slot = slot();
        if let Some(Slot { dirty, notify }) = slot
            && dirty.load(Ordering::Acquire)
        {
            // Veto: AppKit keeps the app running; the notification raises
            // the Save/Discard/Cancel guard on the UI thread.
            notify();
            return TERMINATE_CANCEL;
        }
        let original = ORIGINAL.load(Ordering::Acquire);
        // Self-recursion refusal: `original` is the pre-swap IMP, never this
        // function — forwarding to ourselves recursed on every clean quit.
        if original != 0 && original != self_imp() {
            // SAFETY: `original` is the IMP captured before the swap — the
            // delegate's own `applicationShouldTerminate:`, same ABI.
            let f: unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize =
                unsafe { std::mem::transmute(original) };
            unsafe { f(this, cmd, sender) }
        } else {
            TERMINATE_NOW
        }
    }

    /// This module's `should_terminate` IMP address — the one value a swizzle
    /// must never capture as the original.
    fn self_imp() -> usize {
        should_terminate as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize as usize
    }

    /// The IMP `selector` resolves to on `class`'s superclasses — the real
    /// original when [`should_terminate`] already answers on `class` itself.
    ///
    /// # Safety
    ///
    /// `class` must be a live runtime class; `selector` any valid selector.
    unsafe fn inherited_original(class: *const AnyClass, selector: Sel) -> usize {
        // SAFETY: caller guarantees `class` is a live class on the main thread.
        unsafe {
            let mut ancestor = class_getSuperclass(class);
            while !ancestor.is_null() {
                if let Some(method) = (&*ancestor).instance_method(selector) {
                    let imp = method.implementation() as usize;
                    if imp != self_imp() {
                        return imp;
                    }
                }
                ancestor = class_getSuperclass(ancestor);
            }
            0
        }
    }

    /// Place [`should_terminate`] on `class`'s `selector`, recording the IMP
    /// that answered before in `ORIGINAL` and marking `SWAPPED`.
    ///
    /// The lookup uses `class_getInstanceMethod` semantics — real method
    /// lists only — not `class_getMethodImplementation`, which also returns
    /// the msg-forward trampoline for selectors nothing implements. AppKit
    /// calls the delegate hook only when it truly responds, so wrapping a
    /// trampoline would install a hook where none ran before.
    ///
    /// `class_replaceMethod` returns the displaced IMP when `class` implements
    /// `selector` itself, but returns NULL when it ADDS the method — the
    /// inherited-only case, where the IMP resolved *before* the swap is the
    /// real original to forward to. A NULL return is therefore success, not
    /// failure: treating it as failure left our IMP installed with no state,
    /// and the next attempt captured our own IMP as the original, recursing
    /// on every clean quit.
    ///
    /// # Safety
    ///
    /// `class` must be a live class (the app delegate's) and `selector` a
    /// valid selector; call on the main thread like the rest of the hook.
    unsafe fn swizzle(class: *const AnyClass, selector: Sel) {
        // SAFETY: caller guarantees `class`/`selector` are valid; the runtime
        // calls below only read or add/replace one method entry.
        unsafe {
            let Some(method) = (&*class).instance_method(selector) else {
                // Nothing implements the selector yet — the delegate or
                // Rune's hook isn't ready; a later poll retries.
                return;
            };
            let existing = method.implementation() as usize;
            if existing == self_imp() {
                // Our IMP already answers: a previous swizzle landed without
                // its state, or this class inherited it. Recover the real
                // original from the superclass chain — never store our own
                // IMP, which forwarded clean quits into infinite recursion.
                let original = inherited_original(class, selector);
                if original != 0 {
                    ORIGINAL.store(original, Ordering::Release);
                }
                SWAPPED.store(true, Ordering::Release);
                return;
            }
            let imp =
                should_terminate as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize;
            let previous =
                class_replaceMethod(class, selector, imp as *const c_void, c"Q@:@".as_ptr());
            let original = if previous.is_null() {
                existing
            } else {
                previous as usize
            };
            ORIGINAL.store(original, Ordering::Release);
            SWAPPED.store(true, Ordering::Release);
        }
    }

    pub(super) fn installed() -> bool {
        SWAPPED.load(Ordering::Acquire)
    }

    pub(super) fn install() {
        if SWAPPED.load(Ordering::Acquire) {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: main thread; the delegate and its class outlive the app.
        unsafe {
            let delegate: *mut AnyObject = msg_send![&app, delegate];
            let Some(delegate) = delegate.as_ref() else {
                return;
            };
            let class = delegate.class();
            let selector = sel!(applicationShouldTerminate:);
            swizzle(class, selector);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{ORIGINAL, SWAPPED, TERMINATE_CANCEL, self_imp, should_terminate, swizzle};
        use crate::editor::quit_guard::{SLOT, Slot};
        use objc2::runtime::{AnyClass, AnyObject, Sel};
        use objc2::{msg_send, sel};
        use std::ffi::{CString, c_char, c_void};
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};

        /// ORIGINAL/SWAPPED/SLOT are process-global; the runtime tests share
        /// them, so they run one at a time.
        static LOCK: Mutex<()> = Mutex::new(());

        /// Stand-ins for the delegate's `applicationShouldTerminate:` replies.
        const SENTINEL_BASE: usize = 42;
        const SENTINEL_OWN: usize = 77;

        static BASE_CALLS: AtomicUsize = AtomicUsize::new(0);
        static OWN_CALLS: AtomicUsize = AtomicUsize::new(0);
        static NOTIFY_CALLS: AtomicUsize = AtomicUsize::new(0);

        unsafe extern "C" {
            fn objc_allocateClassPair(
                superclass: *const AnyClass,
                name: *const c_char,
                extra_bytes: usize,
            ) -> *mut AnyClass;
            fn objc_registerClassPair(cls: *mut AnyClass);
            fn class_addMethod(
                class: *mut AnyClass,
                selector: Sel,
                imp: *const c_void,
                types: *const c_char,
            ) -> c_char;
            fn class_createInstance(cls: *const AnyClass, extra_bytes: usize) -> *mut AnyObject;
            fn class_getMethodImplementation(class: *const AnyClass, selector: Sel) -> *mut c_void;
            fn class_replaceMethod(
                class: *const AnyClass,
                selector: Sel,
                imp: *const c_void,
                types: *const c_char,
            ) -> *mut c_void;
        }

        extern "C" fn base_should_terminate(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> usize {
            BASE_CALLS.fetch_add(1, Ordering::SeqCst);
            SENTINEL_BASE
        }

        extern "C" fn own_should_terminate(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> usize {
            OWN_CALLS.fetch_add(1, Ordering::SeqCst);
            SENTINEL_OWN
        }

        struct Fixture {
            /// Implements `applicationShouldTerminate:` itself → SENTINEL_BASE.
            base: *mut AnyClass,
            /// Only inherits the selector from `base`.
            child: *mut AnyClass,
            /// Implements the selector itself → SENTINEL_OWN.
            own: *mut AnyClass,
            /// Implements nothing the swizzle cares about.
            empty: *mut AnyClass,
        }

        /// Register a class on the real runtime; `imp` becomes its
        /// `applicationShouldTerminate:` when given.
        ///
        /// # Safety
        ///
        /// `superclass` must be a live registered class; `name` must be unique
        /// (callers pass a per-test `tag`).
        unsafe fn define_class(
            name: &str,
            superclass: *const AnyClass,
            imp: Option<extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize>,
        ) -> *mut AnyClass {
            let cname = CString::new(name).unwrap();
            // SAFETY: caller guarantees a live superclass and unique name.
            unsafe {
                let cls = objc_allocateClassPair(superclass, cname.as_ptr(), 0);
                assert!(!cls.is_null(), "objc_allocateClassPair({name}) failed");
                if let Some(imp) = imp {
                    let added = class_addMethod(
                        cls,
                        sel!(applicationShouldTerminate:),
                        imp as *const c_void,
                        c"Q@:@".as_ptr(),
                    );
                    assert!(added != 0, "class_addMethod({name}) failed");
                }
                objc_registerClassPair(cls);
                cls
            }
        }

        fn fixture(tag: &str) -> Fixture {
            let ns = AnyClass::get("NSObject").expect("NSObject is registered");
            // SAFETY: `tag` makes every class name unique to this test; the
            // classes stay registered for the rest of the test run.
            unsafe {
                let base = define_class(
                    &format!("GokoQuitGuard{tag}Base"),
                    ns,
                    Some(base_should_terminate),
                );
                let child = define_class(&format!("GokoQuitGuard{tag}Child"), base, None);
                let own = define_class(
                    &format!("GokoQuitGuard{tag}Own"),
                    ns,
                    Some(own_should_terminate),
                );
                let empty = define_class(&format!("GokoQuitGuard{tag}Empty"), ns, None);
                Fixture {
                    base,
                    child,
                    own,
                    empty,
                }
            }
        }

        fn reset_state() {
            SWAPPED.store(false, Ordering::SeqCst);
            ORIGINAL.store(0, Ordering::SeqCst);
            BASE_CALLS.store(0, Ordering::SeqCst);
            OWN_CALLS.store(0, Ordering::SeqCst);
            NOTIFY_CALLS.store(0, Ordering::SeqCst);
            *SLOT.get_or_init(|| Mutex::new(None)).lock().unwrap() = None;
            crate::editor::quit_guard::TEST_SLOT.with(|s| *s.borrow_mut() = None);
        }

        fn set_slot(dirty: bool) {
            fn bump() {
                NOTIFY_CALLS.fetch_add(1, Ordering::SeqCst);
            }
            let dirty = Arc::new(AtomicBool::new(dirty));
            let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(bump);
            *SLOT.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(Slot { dirty, notify });
        }

        /// Real `objc_msgSend` dispatch of `applicationShouldTerminate:` on a
        /// fresh instance of `cls`.
        fn terminate_reply(cls: *mut AnyClass) -> usize {
            // SAFETY: `cls` is a live registered test class; the instance is
            // leaked like the rest of the fixture. `nil` sender matches the
            // AppKit delegate-call shape.
            unsafe {
                let obj = class_createInstance(cls, 0);
                assert!(!obj.is_null(), "class_createInstance failed");
                let obj: *mut AnyObject = msg_send![obj, init];
                msg_send![obj, applicationShouldTerminate: std::ptr::null_mut::<AnyObject>()]
            }
        }

        #[test]
        fn first_install_on_inherited_method_captures_real_original() {
            let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            reset_state();
            let fx = fixture("Inherited");
            let selector = sel!(applicationShouldTerminate:);
            // SAFETY: live test classes; the swizzle is what production runs.
            unsafe {
                // What dispatch resolves before the swap — the real original.
                let expected = class_getMethodImplementation(fx.child, selector) as usize;
                assert_ne!(expected, 0);
                assert_ne!(expected, self_imp());
                swizzle(fx.child, selector);
                assert!(SWAPPED.load(Ordering::SeqCst));
                assert_eq!(ORIGINAL.load(Ordering::SeqCst), expected);
                // The class's own list now answers with our IMP.
                let now = class_getMethodImplementation(fx.child, selector) as usize;
                assert_eq!(now, self_imp());
            }
        }

        #[test]
        fn second_install_is_harmless_and_never_self_captures() {
            let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            reset_state();
            let fx = fixture("Second");
            let selector = sel!(applicationShouldTerminate:);
            // SAFETY: live test classes.
            unsafe {
                swizzle(fx.child, selector);
                let first = ORIGINAL.load(Ordering::SeqCst);
                assert_ne!(first, 0);
                assert_ne!(first, self_imp());
                // The next poll sees our own IMP answering — it must recover
                // the real original, never store ourselves (the field bug).
                swizzle(fx.child, selector);
                assert!(SWAPPED.load(Ordering::SeqCst));
                assert_eq!(ORIGINAL.load(Ordering::SeqCst), first);
            }
        }

        #[test]
        fn own_method_replace_captures_displaced_imp() {
            let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            reset_state();
            set_slot(false);
            let fx = fixture("Own");
            let selector = sel!(applicationShouldTerminate:);
            // SAFETY: live test classes.
            unsafe {
                let expected = class_getMethodImplementation(fx.own, selector) as usize;
                swizzle(fx.own, selector);
                assert_eq!(ORIGINAL.load(Ordering::SeqCst), expected);
                assert_eq!(terminate_reply(fx.own), SENTINEL_OWN);
                assert_eq!(OWN_CALLS.load(Ordering::SeqCst), 1);
            }
        }

        #[test]
        fn clean_quit_forwards_to_real_original() {
            let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            reset_state();
            set_slot(false);
            let fx = fixture("Forward");
            let selector = sel!(applicationShouldTerminate:);
            // SAFETY: live test classes.
            unsafe {
                swizzle(fx.child, selector);
                // Real dispatch: our IMP runs, sees a clean draft, forwards to
                // the inherited original — Rune's `quit::start()` + Now.
                assert_eq!(terminate_reply(fx.child), SENTINEL_BASE);
                assert_eq!(BASE_CALLS.load(Ordering::SeqCst), 1);
                assert_eq!(OWN_CALLS.load(Ordering::SeqCst), 0);
                assert_eq!(NOTIFY_CALLS.load(Ordering::SeqCst), 0);
            }
        }

        #[test]
        fn dirty_quit_cancels_and_notifies_without_forwarding() {
            let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            reset_state();
            set_slot(true);
            let fx = fixture("Dirty");
            let selector = sel!(applicationShouldTerminate:);
            // SAFETY: live test classes.
            unsafe {
                swizzle(fx.child, selector);
                assert_eq!(terminate_reply(fx.child), TERMINATE_CANCEL);
                assert_eq!(NOTIFY_CALLS.load(Ordering::SeqCst), 1);
                assert_eq!(BASE_CALLS.load(Ordering::SeqCst), 0);
            }
        }

        #[test]
        fn absent_method_is_left_alone() {
            let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            reset_state();
            let fx = fixture("Absent");
            let selector = sel!(applicationShouldTerminate:);
            // SAFETY: live test classes.
            unsafe {
                swizzle(fx.empty, selector);
                assert!(!SWAPPED.load(Ordering::SeqCst));
                assert_eq!(ORIGINAL.load(Ordering::SeqCst), 0);
                // Nothing was added: no real implementation exists to wrap.
                // (`class_getMethodImplementation` would still report the
                // msg-forward trampoline here — `instance_method` does not.)
                assert!((&*fx.empty).instance_method(selector).is_none());
            }
        }

        #[test]
        fn torn_install_recovers_real_original_not_self() {
            let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            reset_state();
            let fx = fixture("Torn");
            let selector = sel!(applicationShouldTerminate:);
            // SAFETY: live test classes.
            unsafe {
                // Reproduce the field state: the old code's first poll added
                // our IMP to the class's own list (inherited → ADD → NULL)
                // but recorded no state; the second poll then saw our own
                // IMP and stored it as ORIGINAL.
                let imp =
                    should_terminate as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize;
                let added =
                    class_replaceMethod(fx.child, selector, imp as *const c_void, c"Q@:@".as_ptr());
                assert!(added.is_null(), "inherited method must ADD, not replace");
                // The install path now sees its own IMP — it must recover the
                // inherited original, never store ourselves.
                swizzle(fx.child, selector);
                let original = ORIGINAL.load(Ordering::SeqCst);
                assert_ne!(original, 0);
                assert_ne!(original, self_imp());
                assert_eq!(
                    original,
                    class_getMethodImplementation(fx.base, selector) as usize
                );
                // Clean quit still forwards to the real original.
                assert_eq!(terminate_reply(fx.child), SENTINEL_BASE);
                assert_eq!(BASE_CALLS.load(Ordering::SeqCst), 1);
            }
        }

        #[test]
        fn an_install_on_another_thread_never_clobbers_the_native_slot() {
            let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            reset_state();
            set_slot(true);
            // A parallel headless `EditorState::new` `install`s a clean slot
            // on its own thread — the race that overwrote the fixture. Its
            // thread-local slot dies with the thread, so the fixture's dirty
            // slot must still win here.
            std::thread::spawn(|| {
                crate::editor::quit_guard::install_test_slot(
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(|| {}),
                );
            })
            .join()
            .unwrap();
            let fx = fixture("Isolate");
            let selector = sel!(applicationShouldTerminate:);
            // SAFETY: live test classes.
            unsafe {
                swizzle(fx.child, selector);
                assert_eq!(terminate_reply(fx.child), TERMINATE_CANCEL);
                assert_eq!(NOTIFY_CALLS.load(Ordering::SeqCst), 1);
                assert_eq!(BASE_CALLS.load(Ordering::SeqCst), 0);
            }
        }
    }
}
