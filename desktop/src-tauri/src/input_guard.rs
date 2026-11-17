//! The two input modes of Give control (Kabil 17 Nov: "you move, the agent waits" by default; "Hands-off" when the person opts in).
//!
//! - Yield (always, the default): any REAL mouse movement, click or key on this computer pauses the agent until the person has been
//!   idle for YIELD_IDLE_MS. /act then answers {ok:false, why: WAITING, retry:true} and the page asks again a little later. The agent's
//!   own injected input never counts (Windows: the LLMHF_INJECTED / LLKHF_INJECTED flag; macOS: our events carry MAGIC in
//!   kCGEventSourceUserData). Nothing is blocked in this mode.
//! - Hands-off (only when the page sends handsOff:true with control on): the person's real mouse and keyboard are dropped and only the
//!   agent's injected input goes through. Escapes, all of them always on:
//!     1. Esc pressed 3 times within ESC_WINDOW_MS: the third press ends hands-off AND control (stopped: /act refuses, /health says so);
//!     2. no agent action for HANDS_OFF_IDLE_MS: input comes back on its own (and stays back for this grant);
//!     3. control ends (/state on:false, the end time passes), the app quits or crashes: the hooks belong to this process and die
//!        with it; the page also ends control when the share stops;
//!     4. the operating system's own escapes are never touched (Ctrl+Alt+Del on Windows cannot be hooked at all; Ctrl+Shift+Esc on
//!        Windows and Cmd+Option+Esc on macOS pass; key-ups and modifier keys always pass so nothing stays stuck down).
//!
//! The decisions are pure functions over plain values (tested below); the hooks (Windows: WH_MOUSE_LL / WH_KEYBOARD_LL on their own
//! thread; macOS: a CGEventTap on its own CFRunLoop thread) only translate events and call `decide`. They are installed only while
//! control is on.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub const YIELD_IDLE_MS: u64 = 4_000;
pub const HANDS_OFF_IDLE_MS: u64 = 600_000; // Kabil 17 Nov: 60 s was too short, the crew often thinks longer between actions
pub const ESC_WINDOW_MS: u64 = 1_500;
pub const ESC_COUNT: usize = 3;
/// Marks the agent's own events on macOS (enigo's event_source_user_data); "VALN".
pub const MAGIC: i64 = 0x5641_4C4E;
pub const WAITING: &str = "the person is using the computer: waiting for them";
pub const STOPPED_ESC: &str = "stopped by the person (Esc Esc Esc)";

/// Milliseconds on a monotonic clock that starts with the process (never 0 after the first call, so 0 can mean "never").
pub fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// Esc presses of the person while hands-off: the third within ESC_WINDOW_MS is the escape.
#[derive(Debug, Default)]
pub struct EscCounter {
    at: Vec<u64>,
}

impl EscCounter {
    /// One Esc key-down at `now`; true when it is the third within the window (the counter then starts over).
    pub fn press(&mut self, now: u64) -> bool {
        self.at.retain(|t| now.saturating_sub(*t) <= ESC_WINDOW_MS);
        self.at.push(now);
        if self.at.len() >= ESC_COUNT {
            self.at.clear();
            return true;
        }
        false
    }
    pub fn clear(&mut self) {
        self.at.clear();
    }
}

/// What the guard knows at one moment (a copy of the atomics, so the decisions are pure).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Snap {
    pub on: bool,
    pub hands_off: bool,
    pub released: bool,
    pub stopped: bool,
    /// ms of the last agent action (or of control starting)
    pub last_agent: u64,
    /// ms of the person's last real input, 0 = none seen
    pub last_user: u64,
    /// the agent's action is being carried out right now (a pointer move made by SetCursorPos may not carry the injected flag)
    pub injecting: bool,
}

/// Is the person's real input being dropped right now?
pub fn hands_off_active(s: &Snap, now: u64) -> bool {
    s.on && s.hands_off && !s.released && !s.stopped && now.saturating_sub(s.last_agent) < HANDS_OFF_IDLE_MS
}

/// Hands-off was asked for and the agent has done nothing for HANDS_OFF_IDLE_MS: give the input back (escape 2).
pub fn idle_release_due(s: &Snap, now: u64) -> bool {
    s.on && s.hands_off && !s.released && !s.stopped && now.saturating_sub(s.last_agent) >= HANDS_OFF_IDLE_MS
}

/// Yield: how long the agent should still wait, or None when it may act (the person idle for YIELD_IDLE_MS, or never seen).
pub fn yield_wait(last_user: u64, now: u64) -> Option<u64> {
    if last_user == 0 {
        return None;
    }
    let idle = now.saturating_sub(last_user);
    (idle < YIELD_IDLE_MS).then(|| YIELD_IDLE_MS - idle)
}

/// macOS without the tap (no Accessibility for it): the system's own "seconds since the last input" also counts our injected
/// events, so it only counts as the person when that last input came clearly after our last injection ended.
pub fn fallback_last_user(now: u64, hid_idle_ms: u64, last_inject_end: u64) -> u64 {
    let at = now.saturating_sub(hid_idle_ms).max(1);
    if last_inject_end != 0 && at <= last_inject_end + 250 {
        return 0;
    }
    at
}

/// One input event as a hook sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Move,
    Down,
    Up,
    Wheel,
    /// `os_escape`: the system's own escape chord (Ctrl+Shift+Esc on Windows, Cmd+Option+Esc on macOS): always passes
    KeyDown { esc: bool, os_escape: bool },
    KeyUp,
    /// Shift / Ctrl / Option / Cmd alone: always pass (they do nothing alone and must never stay stuck)
    Modifier,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decision {
    pub pass: bool,
    /// the person's real input (yield counts it)
    pub user: bool,
    /// the third Esc: end hands-off and control
    pub stop: bool,
}

/// The whole filter: injected events always pass; real ones pass and count as the person unless hands-off is active, then they are
/// dropped, except key-ups, modifiers, the OS escape chord; Esc key-downs are counted (and dropped) for the triple-Esc escape.
pub fn decide(injected: bool, kind: Kind, s: &Snap, now: u64, esc: &mut EscCounter) -> Decision {
    let pass = Decision { pass: true, user: false, stop: false };
    if injected || !s.on || (s.injecting && kind == Kind::Move) {
        return pass;
    }
    if !hands_off_active(s, now) {
        return Decision { pass: true, user: true, stop: false };
    }
    match kind {
        Kind::Up | Kind::KeyUp | Kind::Modifier => pass,
        Kind::KeyDown { os_escape: true, .. } => pass,
        Kind::KeyDown { esc: true, .. } => Decision { pass: false, user: false, stop: esc.press(now) },
        Kind::Move | Kind::Down | Kind::Wheel | Kind::KeyDown { .. } => Decision { pass: false, user: false, stop: false },
    }
}

/// What the person and the banner see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Off,
    Yield,
    HandsOff,
    /// hands-off was on and gave the input back (no agent action for 10 minutes)
    Released,
    Stopped,
}

pub fn phase(s: &Snap, now: u64) -> Phase {
    if !s.on {
        Phase::Off
    } else if s.stopped {
        Phase::Stopped
    } else if hands_off_active(s, now) {
        Phase::HandsOff
    } else if s.hands_off {
        Phase::Released
    } else {
        Phase::Yield
    }
}

/// The live state, shared by the HTTP thread, the watcher and the hook thread. Atomics only on the hook path (a low-level hook
/// that is slow gets removed by Windows); the Esc counter is touched by the hook thread alone.
#[derive(Debug, Default)]
pub struct Guard {
    pub on: AtomicBool,
    pub hands_off: AtomicBool,
    pub released: AtomicBool,
    pub stopped: AtomicBool,
    pub last_agent: AtomicU64,
    pub last_user: AtomicU64,
    pub last_inject_end: AtomicU64,
    pub injecting: AtomicBool,
    /// ms when control ends on its own (the grant's end time + 15 s), 0 = unknown
    pub until: AtomicU64,
    /// the native hooks are up (hands-off can actually block on this computer)
    pub hooked: AtomicBool,
    pub esc: Mutex<EscCounter>,
}

impl Guard {
    pub fn snap(&self) -> Snap {
        Snap {
            on: self.on.load(SeqCst),
            hands_off: self.hands_off.load(SeqCst),
            released: self.released.load(SeqCst),
            stopped: self.stopped.load(SeqCst),
            last_agent: self.last_agent.load(SeqCst),
            last_user: self.last_user.load(SeqCst),
            injecting: self.injecting.load(SeqCst),
        }
    }
    /// Control on (a new grant: a stop or a release from before is forgotten) or off.
    pub fn set(&self, on: bool, hands_off: bool, until_ms: u64) {
        let now = now_ms();
        self.stopped.store(false, SeqCst);
        self.released.store(false, SeqCst);
        self.last_agent.store(now, SeqCst);
        self.until.store(if on { until_ms } else { 0 }, SeqCst);
        self.hands_off.store(on && hands_off, SeqCst);
        self.on.store(on, SeqCst);
        if let Ok(mut e) = self.esc.lock() {
            e.clear();
        }
    }
    pub fn note_inject_start(&self) {
        self.last_agent.store(now_ms(), SeqCst);
        self.injecting.store(true, SeqCst);
    }
    pub fn note_inject_end(&self) {
        let now = now_ms();
        self.injecting.store(false, SeqCst);
        self.last_agent.store(now, SeqCst);
        self.last_inject_end.store(now, SeqCst);
    }
    #[cfg(test)]
    pub fn note_user(&self) {
        self.last_user.store(now_ms(), SeqCst);
    }
    /// One event from a hook: returns whether it passes.
    pub fn on_event(&self, injected: bool, kind: Kind) -> bool {
        let now = now_ms();
        let s = self.snap();
        let d = match self.esc.try_lock() {
            Ok(mut e) => decide(injected, kind, &s, now, &mut e),
            Err(_) => decide(injected, kind, &s, now, &mut EscCounter::default()),
        };
        if d.user {
            self.last_user.store(now, SeqCst);
        }
        if d.stop {
            self.stopped.store(true, SeqCst);
        }
        d.pass
    }
}

/// Windows low-level hook messages as the filter's kinds (pure, so they are tested on any system; used by `win`).
#[cfg_attr(not(windows), allow(dead_code))]
pub mod winmap {
    use super::Kind;

    /// A mouse message (wParam of WH_MOUSE_LL) as the pure filter's kind.
    pub fn mouse_kind(msg: usize) -> Kind {
        match msg {
            0x0201 | 0x0204 | 0x0207 | 0x020B => Kind::Down, // L / R / M / X button down
            0x0202 | 0x0205 | 0x0208 | 0x020C => Kind::Up,
            0x020A | 0x020E => Kind::Wheel, // vertical / horizontal wheel
            _ => Kind::Move,
        }
    }

    /// A key message (wParam of WH_KEYBOARD_LL) and virtual key as the pure filter's kind; `ctrl_shift`: both held (Task Manager).
    pub fn key_kind(msg: usize, vk: u32, ctrl_shift: bool) -> Kind {
        let up = msg == 0x0101 || msg == 0x0105; // WM_KEYUP / WM_SYSKEYUP
        if matches!(vk, 0x10 | 0x11 | 0xA0..=0xA3) {
            return Kind::Modifier; // Shift / Ctrl (not Alt or Win: those act alone)
        }
        if up {
            return Kind::KeyUp;
        }
        let esc = vk == 0x1B;
        Kind::KeyDown { esc, os_escape: esc && ctrl_shift }
    }

    pub fn injected_mouse(flags: u32) -> bool {
        flags & (0x1 | 0x2) != 0
    }
    pub fn injected_key(flags: u32) -> bool {
        flags & (0x10 | 0x2) != 0
    }
}

// ---------------------------------------------------------------- the hooks
/// Installs the hooks for this guard (control on). Returns true when hands-off can block on this computer.
pub fn engage(g: &'static Guard) -> bool {
    native::engage(g)
}

/// Removes the hooks (control off). Safe to call when none are installed.
pub fn disengage() {
    native::disengage()
}

/// The person's last real input when no hook is up (macOS fallback; 0 elsewhere).
pub fn fallback_user(g: &Guard) -> u64 {
    native::fallback_user(g)
}

#[cfg(target_os = "macos")]
use mac as native;
#[cfg(windows)]
use win as native;
#[cfg(not(any(target_os = "macos", windows)))]
mod native {
    pub fn engage(_g: &'static super::Guard) -> bool {
        false
    }
    pub fn disengage() {}
    pub fn fallback_user(_g: &super::Guard) -> u64 {
        0
    }
}

/// Windows: WH_MOUSE_LL and WH_KEYBOARD_LL on a dedicated thread with its own message loop. (`--cfg win_check` type-checks it on a
/// Mac: `RUSTFLAGS="--cfg win_check" cargo check`; it links only on Windows.)
#[cfg(any(windows, win_check))]
#[allow(dead_code, non_snake_case, clippy::upper_case_acronyms)]
mod win {
    use super::winmap::{injected_key, injected_mouse, key_kind, mouse_kind};
    use super::Guard;
    use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering::SeqCst};

    #[repr(C)]
    struct POINT {
        x: i32,
        y: i32,
    }
    #[repr(C)]
    struct MSLLHOOKSTRUCT {
        pt: POINT,
        mouse_data: u32,
        flags: u32,
        time: u32,
        extra: usize,
    }
    #[repr(C)]
    struct KBDLLHOOKSTRUCT {
        vk: u32,
        scan: u32,
        flags: u32,
        time: u32,
        extra: usize,
    }
    #[repr(C)]
    struct MSG {
        hwnd: isize,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt: POINT,
        private: u32,
    }
    type HookProc = unsafe extern "system" fn(i32, usize, isize) -> isize;
    #[link(name = "user32")]
    extern "system" {
        fn SetWindowsHookExW(id: i32, f: HookProc, hmod: isize, tid: u32) -> isize;
        fn UnhookWindowsHookEx(h: isize) -> i32;
        fn CallNextHookEx(h: isize, code: i32, w: usize, l: isize) -> isize;
        fn GetMessageW(msg: *mut MSG, hwnd: isize, min: u32, max: u32) -> i32;
        fn PeekMessageW(msg: *mut MSG, hwnd: isize, min: u32, max: u32, remove: u32) -> i32;
        fn PostThreadMessageW(tid: u32, msg: u32, w: usize, l: isize) -> i32;
        fn GetAsyncKeyState(vk: i32) -> i16;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
        fn GetCurrentThreadId() -> u32;
    }
    const WH_KEYBOARD_LL: i32 = 13;
    const WH_MOUSE_LL: i32 = 14;
    const WM_QUIT: u32 = 0x0012;
    const VK_ESCAPE: u32 = 0x1B;

    static GUARD: AtomicPtr<Guard> = AtomicPtr::new(std::ptr::null_mut());
    static THREAD: AtomicU32 = AtomicU32::new(0);

    unsafe extern "system" fn on_mouse(code: i32, w: usize, l: isize) -> isize {
        let g = GUARD.load(SeqCst);
        if code == 0 && !g.is_null() && l != 0 {
            let m = &*(l as *const MSLLHOOKSTRUCT);
            if !(*g).on_event(injected_mouse(m.flags), mouse_kind(w)) {
                return 1;
            }
        }
        CallNextHookEx(0, code, w, l)
    }

    unsafe extern "system" fn on_key(code: i32, w: usize, l: isize) -> isize {
        let g = GUARD.load(SeqCst);
        if code == 0 && !g.is_null() && l != 0 {
            let k = &*(l as *const KBDLLHOOKSTRUCT);
            let held = |vk: i32| (GetAsyncKeyState(vk) as u16 & 0x8000) != 0;
            let cs = k.vk == VK_ESCAPE && held(0x11) && held(0x10);
            if !(*g).on_event(injected_key(k.flags), key_kind(w, k.vk, cs)) {
                return 1;
            }
        }
        CallNextHookEx(0, code, w, l)
    }

    pub fn engage(g: &'static Guard) -> bool {
        GUARD.store(g as *const Guard as *mut Guard, SeqCst);
        if THREAD.load(SeqCst) != 0 {
            return true;
        }
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        std::thread::spawn(move || unsafe {
            let hmod = GetModuleHandleW(std::ptr::null());
            let hm = SetWindowsHookExW(WH_MOUSE_LL, on_mouse, hmod, 0);
            let hk = SetWindowsHookExW(WH_KEYBOARD_LL, on_key, hmod, 0);
            if hm == 0 || hk == 0 {
                if hm != 0 {
                    UnhookWindowsHookEx(hm);
                }
                if hk != 0 {
                    UnhookWindowsHookEx(hk);
                }
                let _ = tx.send(false);
                return;
            }
            let mut msg: MSG = std::mem::zeroed();
            PeekMessageW(&mut msg, 0, 0, 0, 0); // makes this thread's message queue, so WM_QUIT from disengage() always arrives
            THREAD.store(GetCurrentThreadId(), SeqCst);
            let _ = tx.send(true);
            while GetMessageW(&mut msg, 0, 0, 0) > 0 {}
            UnhookWindowsHookEx(hm);
            UnhookWindowsHookEx(hk);
            THREAD.store(0, SeqCst);
        });
        let ok = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap_or(false);
        g.hooked.store(ok, SeqCst);
        ok
    }

    pub fn disengage() {
        let t = THREAD.load(SeqCst);
        if t != 0 {
            unsafe { PostThreadMessageW(t, WM_QUIT, 0, 0) };
        }
        let g = GUARD.load(SeqCst);
        if !g.is_null() {
            unsafe { (*g).hooked.store(false, SeqCst) };
        }
    }

    pub fn fallback_user(_g: &Guard) -> u64 {
        0
    }
}

/// macOS: a CGEventTap (session tap, active filter) on its own thread with a CFRunLoop. Needs Accessibility, which Give control
/// already asks for; without it the tap is not created, hands-off cannot block, and yield falls back to the system's idle time.
#[cfg(target_os = "macos")]
mod mac {
    use super::{Guard, Kind, MAGIC};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicPtr, Ordering::SeqCst};

    type Cb = extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventTapCreate(tap: u32, place: u32, options: u32, mask: u64, cb: Cb, info: *mut c_void) -> *mut c_void;
        fn CGEventTapEnable(tap: *mut c_void, enable: bool);
        fn CGEventGetIntegerValueField(ev: *mut c_void, field: u32) -> i64;
        fn CGEventGetFlags(ev: *mut c_void) -> u64;
        fn CGEventSourceSecondsSinceLastEventType(state: i32, ty: u32) -> f64;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFMachPortCreateRunLoopSource(alloc: *const c_void, port: *mut c_void, order: isize) -> *mut c_void;
        fn CFMachPortInvalidate(port: *mut c_void);
        fn CFRunLoopGetCurrent() -> *mut c_void;
        fn CFRunLoopAddSource(rl: *mut c_void, src: *mut c_void, mode: *const c_void);
        fn CFRunLoopRun();
        fn CFRunLoopStop(rl: *mut c_void);
        fn CFRelease(p: *const c_void);
        static kCFRunLoopCommonModes: *const c_void;
    }
    const SESSION_TAP: u32 = 1;
    const HEAD: u32 = 0;
    const DEFAULT: u32 = 0; // an active filter: it may drop events
    const USER_DATA: u32 = 42; // kCGEventSourceUserData
    const KEYCODE: u32 = 9; // kCGKeyboardEventKeycode
    const ESC: i64 = 53;
    const CMD: u64 = 0x10_0000;
    const OPT: u64 = 0x8_0000;
    const DISABLED_TIMEOUT: u32 = 0xFFFF_FFFE;
    const DISABLED_USER: u32 = 0xFFFF_FFFF;

    static GUARD: AtomicPtr<Guard> = AtomicPtr::new(std::ptr::null_mut());
    static TAP: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
    static LOOP: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

    /// A CGEventType as the pure filter's kind (None: not an input event we look at).
    pub fn kind_of(ty: u32, keycode: i64, flags: u64) -> Option<Kind> {
        Some(match ty {
            5 | 6 | 7 | 27 => Kind::Move, // moved, left / right / other dragged
            1 | 3 | 25 => Kind::Down,
            2 | 4 | 26 => Kind::Up,
            22 => Kind::Wheel,
            10 => {
                let esc = keycode == ESC;
                Kind::KeyDown { esc, os_escape: esc && flags & CMD != 0 && flags & OPT != 0 }
            }
            11 => Kind::KeyUp,
            12 => Kind::Modifier, // flagsChanged
            _ => return None,
        })
    }

    pub fn mask() -> u64 {
        [1u32, 2, 3, 4, 5, 6, 7, 10, 11, 12, 22, 25, 26, 27].iter().fold(0u64, |m, t| m | (1u64 << t))
    }

    extern "C" fn on_event(_proxy: *mut c_void, ty: u32, ev: *mut c_void, _info: *mut c_void) -> *mut c_void {
        if ty == DISABLED_TIMEOUT || ty == DISABLED_USER {
            let t = TAP.load(SeqCst);
            if !t.is_null() {
                unsafe { CGEventTapEnable(t, true) };
            }
            return ev;
        }
        let g = GUARD.load(SeqCst);
        if g.is_null() || ev.is_null() {
            return ev;
        }
        let (keycode, flags, data) = unsafe { (CGEventGetIntegerValueField(ev, KEYCODE), CGEventGetFlags(ev), CGEventGetIntegerValueField(ev, USER_DATA)) };
        let Some(k) = kind_of(ty, keycode, flags) else { return ev };
        if unsafe { (*g).on_event(data == MAGIC, k) } {
            ev
        } else {
            std::ptr::null_mut()
        }
    }

    struct Send(*mut c_void);
    unsafe impl std::marker::Send for Send {}

    pub fn engage(g: &'static Guard) -> bool {
        GUARD.store(g as *const Guard as *mut Guard, SeqCst);
        if !LOOP.load(SeqCst).is_null() {
            return g.hooked.load(SeqCst);
        }
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        std::thread::spawn(move || unsafe {
            let tap = CGEventTapCreate(SESSION_TAP, HEAD, DEFAULT, mask(), on_event, std::ptr::null_mut());
            if tap.is_null() {
                let _ = tx.send(false);
                return;
            }
            let src = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
            let rl = CFRunLoopGetCurrent();
            CFRunLoopAddSource(rl, src, kCFRunLoopCommonModes);
            TAP.store(tap, SeqCst);
            LOOP.store(rl, SeqCst);
            CGEventTapEnable(tap, true);
            let _ = tx.send(true);
            CFRunLoopRun();
            CGEventTapEnable(tap, false);
            TAP.store(std::ptr::null_mut(), SeqCst);
            LOOP.store(std::ptr::null_mut(), SeqCst);
            CFMachPortInvalidate(tap);
            CFRelease(src);
            CFRelease(tap);
        });
        let ok = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap_or(false);
        g.hooked.store(ok, SeqCst);
        ok
    }

    pub fn disengage() {
        let rl = Send(LOOP.load(SeqCst));
        if !rl.0.is_null() {
            unsafe { CFRunLoopStop(rl.0) };
        }
        let g = GUARD.load(SeqCst);
        if !g.is_null() {
            unsafe { (*g).hooked.store(false, SeqCst) };
        }
    }

    /// Seconds since the last real-or-synthetic input of the HID system, read as the person's input only when it came after our
    /// last injection (super::fallback_last_user).
    pub fn fallback_user(g: &Guard) -> u64 {
        let s = unsafe { CGEventSourceSecondsSinceLastEventType(1, u32::MAX) }; // HIDSystemState, kCGAnyInputEventType
        if !s.is_finite() || s < 0.0 {
            return 0;
        }
        super::fallback_last_user(super::now_ms(), (s * 1000.0) as u64, g.last_inject_end.load(SeqCst))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn mac_events_map_to_kinds() {
            assert_eq!(kind_of(5, 0, 0), Some(Kind::Move));
            assert_eq!(kind_of(1, 0, 0), Some(Kind::Down));
            assert_eq!(kind_of(2, 0, 0), Some(Kind::Up));
            assert_eq!(kind_of(22, 0, 0), Some(Kind::Wheel));
            assert_eq!(kind_of(10, 53, 0), Some(Kind::KeyDown { esc: true, os_escape: false }));
            assert_eq!(kind_of(10, 53, CMD | OPT), Some(Kind::KeyDown { esc: true, os_escape: true }), "Force Quit passes");
            assert_eq!(kind_of(10, 0, CMD), Some(Kind::KeyDown { esc: false, os_escape: false }));
            assert_eq!(kind_of(12, 56, 0), Some(Kind::Modifier));
            assert_eq!(kind_of(14, 0, 0), None, "system-defined (media keys, power) is not looked at");
            assert_eq!(mask() & (1 << 10), 1 << 10);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(hands_off: bool, last_agent: u64) -> Snap {
        Snap { on: true, hands_off, released: false, stopped: false, last_agent, last_user: 0, injecting: false }
    }

    #[test]
    fn three_escs_within_the_window_and_only_then() {
        let mut e = EscCounter::default();
        assert!(!e.press(1_000));
        assert!(!e.press(1_400));
        assert!(e.press(2_400), "the third within 1.5 s of the first");
        let mut e = EscCounter::default();
        assert!(!e.press(1_000));
        assert!(!e.press(2_000));
        assert!(!e.press(2_600), "the first is 1.6 s old: only two count");
        assert!(e.press(2_700));
        assert!(!e.press(2_800), "it starts over after an escape");
    }

    #[test]
    fn hands_off_lets_go_after_a_minute_without_the_agent() {
        let s = on(true, 10_000);
        assert!(hands_off_active(&s, 10_000 + HANDS_OFF_IDLE_MS - 1));
        assert!(!hands_off_active(&s, 10_000 + HANDS_OFF_IDLE_MS));
        assert!(idle_release_due(&s, 10_000 + HANDS_OFF_IDLE_MS));
        assert!(!idle_release_due(&on(false, 0), 10_000 + HANDS_OFF_IDLE_MS), "yield only: nothing to release");
        assert_eq!(phase(&s, 20_000), Phase::HandsOff);
        assert_eq!(phase(&s, 10_000 + HANDS_OFF_IDLE_MS), Phase::Released);
        assert_eq!(phase(&Snap { released: true, ..s }, 20_000), Phase::Released, "once released it stays released");
        assert_eq!(phase(&Snap { stopped: true, ..s }, 20_000), Phase::Stopped);
        assert_eq!(phase(&on(false, 0), 1), Phase::Yield);
        assert_eq!(phase(&Snap::default(), 1), Phase::Off);
        assert!(!hands_off_active(&Snap { on: false, ..s }, 20_000), "never blocks when control is off");
    }

    #[test]
    fn the_agent_waits_while_the_person_works() {
        assert_eq!(yield_wait(0, 50_000), None, "never seen: go");
        assert_eq!(yield_wait(50_000, 51_000), Some(3_000));
        assert_eq!(yield_wait(50_000, 50_000 + YIELD_IDLE_MS), None);
    }

    #[test]
    fn the_idle_fallback_ignores_our_own_injection() {
        assert_eq!(fallback_last_user(100_000, 500, 0), 99_500, "no injection yet: the last input is the person");
        assert_eq!(fallback_last_user(100_000, 500, 99_400), 0, "it came with our injection: not the person");
        assert_eq!(fallback_last_user(100_000, 500, 90_000), 99_500, "long after our injection: the person");
    }

    #[test]
    fn the_filter() {
        let mut e = EscCounter::default();
        let ho = on(true, 1_000);
        let yi = on(false, 1_000);
        let (p, d) = (|x: Decision| x.pass, |x: Decision| x.user);
        // the agent's own input always passes and is never the person
        for s in [ho, yi, Snap::default()] {
            let x = decide(true, Kind::Down, &s, 2_000, &mut e);
            assert!(p(x) && !d(x));
        }
        // control off: everything passes, nothing counted
        let x = decide(false, Kind::Down, &Snap::default(), 2_000, &mut e);
        assert!(p(x) && !d(x));
        // yield: real input passes and counts
        for k in [Kind::Move, Kind::Down, Kind::Wheel, Kind::KeyDown { esc: false, os_escape: false }] {
            let x = decide(false, k, &yi, 2_000, &mut e);
            assert!(p(x) && d(x), "{k:?}");
        }
        // hands-off: real input is dropped
        for k in [Kind::Move, Kind::Down, Kind::Wheel, Kind::KeyDown { esc: false, os_escape: false }] {
            assert!(!p(decide(false, k, &ho, 2_000, &mut e)), "{k:?}");
        }
        // ...but ups and modifiers pass (nothing stuck down) and the OS escape chord passes
        for k in [Kind::Up, Kind::KeyUp, Kind::Modifier, Kind::KeyDown { esc: true, os_escape: true }] {
            assert!(p(decide(false, k, &ho, 2_000, &mut e)), "{k:?}");
        }
        // Esc x3: counted, the third stops
        let esc = Kind::KeyDown { esc: true, os_escape: false };
        let mut e = EscCounter::default();
        assert!(!decide(false, esc, &ho, 2_000, &mut e).stop);
        assert!(!decide(false, esc, &ho, 2_300, &mut e).stop);
        let x = decide(false, esc, &ho, 2_600, &mut e);
        assert!(x.stop && !x.pass);
        // a pointer move while the agent's action runs is the agent's (Windows SetCursorPos), a click is not
        let busy = Snap { injecting: true, ..yi };
        assert!(!d(decide(false, Kind::Move, &busy, 2_000, &mut e)));
        assert!(d(decide(false, Kind::Down, &busy, 2_000, &mut e)));
        // 10 minutes without the agent: input is back
        assert!(p(decide(false, Kind::Down, &ho, 1_000 + HANDS_OFF_IDLE_MS, &mut e)));
    }

    #[test]
    fn the_guard_stops_on_three_escs_and_forgets_it_on_a_new_grant() {
        let g = Guard::default();
        g.set(true, true, 0);
        assert_eq!(phase(&g.snap(), now_ms()), Phase::HandsOff);
        let esc = Kind::KeyDown { esc: true, os_escape: false };
        assert!(!g.on_event(false, Kind::Move));
        assert!(g.on_event(true, Kind::Move), "the agent's input");
        assert!(!g.on_event(false, esc));
        assert!(!g.on_event(false, esc));
        assert!(!g.on_event(false, esc));
        assert_eq!(phase(&g.snap(), now_ms()), Phase::Stopped);
        assert!(g.on_event(false, Kind::Move), "stopped: the person has the mouse again");
        assert_ne!(g.last_user.load(SeqCst), 0);
        g.set(true, false, 0);
        assert_eq!(phase(&g.snap(), now_ms()), Phase::Yield);
        g.set(false, true, 0);
        assert_eq!(phase(&g.snap(), now_ms()), Phase::Off);
        assert!(!g.hands_off.load(SeqCst), "hands-off never outlives control");
    }

    #[test]
    fn windows_messages_map_to_kinds() {
        use super::winmap::*;
        assert_eq!(mouse_kind(0x0200), Kind::Move);
        assert_eq!(mouse_kind(0x0201), Kind::Down);
        assert_eq!(mouse_kind(0x0202), Kind::Up);
        assert_eq!(mouse_kind(0x020A), Kind::Wheel);
        assert_eq!(key_kind(0x0100, 0x1B, false), Kind::KeyDown { esc: true, os_escape: false });
        assert_eq!(key_kind(0x0100, 0x1B, true), Kind::KeyDown { esc: true, os_escape: true }, "Ctrl+Shift+Esc passes");
        assert_eq!(key_kind(0x0101, 0x41, false), Kind::KeyUp);
        assert_eq!(key_kind(0x0100, 0xA2, false), Kind::Modifier);
        assert_eq!(key_kind(0x0104, 0x12, false), Kind::KeyDown { esc: false, os_escape: false }, "Alt alone is held back");
        assert!(injected_mouse(0x1) && !injected_mouse(0));
        assert!(injected_key(0x10) && !injected_key(0x80));
    }
}
