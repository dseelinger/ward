//! Reading the physical keyboard, whoever it is actually typing to.
//!
//! Ward's panel lives in a headset, and a headset has no window focus. Elite has it, or the
//! desktop does, and neither is going to forward a keystroke to an overlay. So the keyboard is
//! read the same way the push-to-talk key already is — asked about directly rather than waited
//! for — and the difference is only that this asks about all of them.
//!
//! # It watches, and does not intercept
//!
//! Every key read here still reaches whatever has focus. That is a deliberate limit rather than an
//! oversight: taking a key away from the game means a hook that can fail with the keyboard still
//! swallowed, and a Commander who cannot fly because Ward stopped forwarding their keys is a much
//! worse failure than one who has to click away from a text box.
//!
//! What it means in practice is that typing into the panel while the game has focus also types
//! into the game. Typing into the panel from the SteamVR dashboard, or with Ward's own window in
//! front, does not.
//!
//! # Nothing is typed until something is listening
//!
//! The state is primed on the first pass after a box takes focus, rather than compared against
//! whatever was held before. Otherwise a key already down when the Commander pointed at the box —
//! the key they pressed to summon the panel, most obviously — would arrive as the first character
//! they never typed.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, GetKeyboardLayout, MAPVK_VK_TO_VSC, MapVirtualKeyW, ToUnicodeEx,
    VK_CAPITAL, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

/// How many virtual keys there are. Windows numbers them in a byte.
const KEYS: usize = 256;

/// The lowest virtual key worth asking about. Below this are the mouse buttons, which are not
/// typing and would arrive as nothing anyway.
const FIRST: usize = 0x08;

/// The three letters that mean something with control held. Virtual keys are the ASCII capitals
/// for letters, whatever the layout prints on them - so these are the keys in the places X, C and
/// V sit on a US keyboard, which is where every layout puts these three shortcuts anyway.
const CUT: u16 = b'X' as u16;
const COPY: u16 = b'C' as u16;
const PASTE: u16 = b'V' as u16;

/// One thing the Commander did to a text box.
#[derive(Clone, Debug, PartialEq)]
pub enum Typed {
    /// Characters, already in the letters their own layout produces.
    Text(String),
    /// Ctrl+C, Ctrl+X, Ctrl+V. The three shortcuts anybody typing into a box expects, and the
    /// only ones held out of the rule that a modifier means a shortcut for somebody else.
    Copy,
    Cut,
    /// Carries what was on the clipboard, read at the moment it was asked for.
    Paste(String),
}

/// Reads what has been typed since the last time it was asked.
pub struct Keyboard {
    /// What was down last pass, so a key held across two of them is not typed twice.
    down: [bool; KEYS],
    /// Whether the state above is worth comparing against. False until a pass has primed it.
    primed: bool,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self {
            down: [false; KEYS],
            primed: false,
        }
    }
}

impl Keyboard {
    /// What was typed since the last call, or nothing if nobody is listening.
    ///
    /// `listening` is whether anything on the panel actually wants text. When it goes false the
    /// reader forgets what it saw, so the next box to take focus starts from a clean slate rather
    /// than from keys held while nothing was being typed into.
    pub fn typed(&mut self, listening: bool) -> Vec<Typed> {
        if !listening {
            self.primed = false;
            return Vec::new();
        }

        let now = read_all();

        // The first pass after focus arrives records and says nothing. Everything held at that
        // moment was pressed for some other reason.
        if !self.primed {
            self.down = now;
            self.primed = true;
            return Vec::new();
        }

        let mut typed = Vec::new();

        for key in fresh(&now, &self.down) {
            typed.extend(what(key));
        }

        self.down = now;
        typed
    }
}

/// Which keys are newly down, in the order Windows numbers them.
///
/// Split out because it is the whole of the edge detection and the only part of this file that can
/// be tested: everything else is asking the operating system a question.
fn fresh(now: &[bool; KEYS], before: &[bool; KEYS]) -> Vec<u16> {
    (FIRST..KEYS)
        .filter(|key| now[*key] && !before[*key])
        .map(|key| {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the range ends at 256, which is what a u16 is being narrowed from"
            )]
            let key = key as u16;
            key
        })
        .collect()
}

/// Every key's state right now.
fn read_all() -> [bool; KEYS] {
    let mut down = [false; KEYS];

    for (key, held) in down.iter_mut().enumerate().skip(FIRST) {
        // The high bit is "down now". The low bit is "was pressed since last asked", which is a
        // different question and the wrong one here for the same reason it is wrong for the
        // push-to-talk key: it reports a tap that has already ended as still being held.
        #[expect(
            clippy::cast_possible_wrap,
            reason = "the range ends at 256 and the call takes a signed key"
        )]
        let state = unsafe { GetAsyncKeyState(key as i32) };
        *held = (state as u16 & 0x8000) != 0;
    }

    down
}

/// What one key press means: a character, or one of the three clipboard shortcuts.
fn what(key: u16) -> Option<Typed> {
    let held = |key: u16| unsafe { GetAsyncKeyState(i32::from(key)) } as u16 & 0x8000 != 0;

    // Control is a shortcut for somebody else, with exactly three exceptions - and they are the
    // three that anybody who has ever used a text box will try first. Alt is never an exception:
    // there is nothing an overlay wants from it that is worth taking alt-tab away for.
    if held(VK_CONTROL) {
        if held(VK_MENU) {
            return None;
        }

        return match key {
            CUT => Some(Typed::Cut),
            COPY => Some(Typed::Copy),
            PASTE => Some(Typed::Paste(clipboard::read().unwrap_or_default())),
            _ => None,
        };
    }

    let text = character(key);

    match text.is_empty() {
        true => None,
        false => Some(Typed::Text(text)),
    }
}

/// What one key produces, on this Commander's own layout.
///
/// Translated rather than mapped from a table, because a table is a table for one keyboard. The
/// same physical key is a different character on an AZERTY layout, and the Commander typing on one
/// means the letter they see printed on it.
fn character(key: u16) -> String {
    let mut state = [0u8; KEYS];

    // Only the modifiers, and read directly rather than through `GetKeyboardState` - that reports
    // the focused thread's queue, and the focused thread is the game.
    for modifier in [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN] {
        if unsafe { GetAsyncKeyState(i32::from(modifier)) } as u16 & 0x8000 != 0 {
            state[modifier as usize] = 0x80;
        }
    }

    // Caps lock is a latch rather than a hold, so it is the low bit that says whether it is on.
    if unsafe { GetKeyState(i32::from(VK_CAPITAL)) } & 1 != 0 {
        state[VK_CAPITAL as usize] = 1;
    }

    // A held control or alt is a shortcut rather than a letter. Without this, Ctrl+C types a
    // control code into whatever box has focus.
    if state[VK_CONTROL as usize] != 0 || state[VK_MENU as usize] != 0 {
        return String::new();
    }

    let scancode = unsafe { MapVirtualKeyW(u32::from(key), MAPVK_VK_TO_VSC) };
    let layout = unsafe { GetKeyboardLayout(0) };
    let mut out = [0u16; 8];

    #[expect(
        clippy::cast_possible_truncation,
        reason = "the buffer is 8 long, which an i32 counts easily"
    )]
    let room = out.len() as i32;

    // The last argument keeps the keyboard's own dead-key state untouched, so typing an accent
    // into Ward does not leave the next character the Commander types elsewhere combined with it.
    let written = unsafe {
        ToUnicodeEx(
            u32::from(key),
            scancode,
            state.as_ptr(),
            out.as_mut_ptr(),
            room,
            1 << 2,
            layout,
        )
    };

    if written <= 0 {
        return String::new();
    }

    #[expect(clippy::cast_sign_loss, reason = "checked positive immediately above")]
    let written = written as usize;

    String::from_utf16_lossy(&out[..written.min(out.len())])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> [bool; KEYS] {
        [false; KEYS]
    }

    #[test]
    fn a_key_that_has_just_gone_down_is_the_only_one_reported() {
        let before = none();
        let mut now = none();
        now[0x41] = true;

        assert_eq!(fresh(&now, &before), vec![0x41]);
    }

    #[test]
    fn a_key_held_across_two_passes_is_typed_once() {
        // The failure this prevents is the one everybody has seen: a letter repeating for as long
        // as a finger rests on it, at whatever rate the loop happens to run.
        let mut held = none();
        held[0x41] = true;

        assert_eq!(fresh(&held, &held), Vec::<u16>::new());
    }

    #[test]
    fn releasing_a_key_types_nothing() {
        let mut before = none();
        before[0x41] = true;

        assert_eq!(fresh(&none(), &before), Vec::<u16>::new());
    }

    #[test]
    fn the_mouse_buttons_are_not_typing() {
        // They are virtual keys 1 to 7, and they would otherwise be read every time somebody
        // clicked something.
        let before = none();
        let mut now = none();
        now[0x01] = true;
        now[0x02] = true;

        assert_eq!(fresh(&now, &before), Vec::<u16>::new());
    }

    #[test]
    fn nothing_is_typed_until_something_is_listening() {
        // A box takes focus while the summoning key is still held. That key was pressed to open
        // the panel, not to be typed into it.
        let mut keyboard = Keyboard::default();

        assert!(keyboard.typed(false).is_empty());
        assert!(
            !keyboard.primed,
            "it should not be comparing against anything"
        );
    }

    #[test]
    fn focus_going_away_forgets_what_was_held() {
        // Otherwise a key pressed while the panel had no text box would arrive as the first
        // character typed into the next one that did.
        let mut down = [false; KEYS];
        down[0x41] = true;

        let mut keyboard = Keyboard { down, primed: true };

        keyboard.typed(false);

        assert!(!keyboard.primed);
    }
}

/// Taking the keyboard away from whatever else is in front, while a box is being typed into.
///
/// A low-level hook is the only thing on Windows that can stop a key reaching the application with
/// focus. It is installed while a panel text box has focus and removed the moment one does not, so
/// Ward is in the keyboard's path for exactly as long as somebody is typing at it and no longer.
///
/// # What is never taken
///
/// The push-to-talk key and the key that summons the panel always pass through, and so does
/// anything held with control or alt. A Commander who cannot talk to Ward or dismiss its panel
/// because a text box has focus is trapped by the very thing meant to be helping them, and that is
/// a worse outcome than a stray letter reaching the game.
///
/// # Why its own thread
///
/// The hook is called on the thread that installed it, and that thread must be pumping messages.
/// Windows gives the callback a few hundred milliseconds before it silently removes the hook, and
/// every keystroke on the machine waits on it in the meantime — so it cannot live on a thread that
/// also draws a frame or waits on a lock. This one does nothing else at all.
///
/// The timeout is worth knowing about the right way round: when this goes wrong, Windows takes the
/// hook away and keys flow normally again. The failure is Ward stopping, not the keyboard sticking.
pub mod steal {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use super::{COPY, CUT, PASTE};
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_MENU};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, MSG, SetWindowsHookExW,
        TranslateMessage, WH_KEYBOARD_LL,
    };

    /// Whether keys are currently being taken.
    ///
    /// Read by the hook and written by the overlay, which is why it is an atomic rather than
    /// anything that could block: the hook runs on every keystroke on the machine and must never
    /// wait for a lock somebody else is holding.
    static TAKING: AtomicBool = AtomicBool::new(false);

    /// The two keys that are never taken, however they are set.
    ///
    /// Held as numbers rather than names because the hook cannot read a settings file, and as
    /// atomics for the same reason `TAKING` is one.
    static SPARE_ONE: AtomicU32 = AtomicU32::new(0);
    static SPARE_TWO: AtomicU32 = AtomicU32::new(0);

    /// Starts the thread the hook lives on. Called once.
    pub fn start() {
        let started = std::thread::Builder::new()
            .name("ward-keyboard".to_string())
            .spawn(pump);

        if let Err(e) = started {
            tracing::warn!(
                target: "ward::keys",
                error = %e,
                "the keyboard cannot be taken from the game, and typing will reach both"
            );
        }
    }

    /// Which keys must always reach whatever else is listening.
    pub fn spare(keys: [u16; 2]) {
        SPARE_ONE.store(u32::from(keys[0]), Ordering::Relaxed);
        SPARE_TWO.store(u32::from(keys[1]), Ordering::Relaxed);
    }

    /// Whether to take keys from now on.
    pub fn taking(now: bool) {
        // Swapped rather than stored, so the change is logged once rather than every frame.
        if TAKING.swap(now, Ordering::Relaxed) != now {
            tracing::info!(target: "ward::keys", taking = now, "keyboard");
        }
    }

    /// Installs the hook and does nothing else, forever.
    ///
    /// The message loop is not decoration. A low-level hook is delivered to the thread that
    /// installed it by posting to its queue, so a thread that never pumps is a hook that is never
    /// called — and Windows removes it after the timeout, which reads as the feature simply not
    /// working.
    fn pump() {
        let hook =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(watch), std::ptr::null_mut(), 0) };

        if hook.is_null() {
            tracing::warn!(
                target: "ward::keys",
                "Windows refused the keyboard hook, so typing will reach the game as well"
            );
            return;
        }

        tracing::info!(target: "ward::keys", "keyboard hook installed");

        let mut message = MSG::default();

        // Runs until the process ends. There is no path that removes the hook, because the only
        // reason to remove it is Ward closing - and closing takes the thread with it.
        while unsafe { GetMessageW(&raw mut message, std::ptr::null_mut(), 0, 0) } > 0 {
            unsafe {
                let _ = TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }
    }

    /// Called for every key on the machine. Returns non-zero to swallow one.
    ///
    /// Everything here is a comparison against an atomic. Nothing allocates, nothing locks and
    /// nothing waits, because whatever this does is done between the Commander pressing a key and
    /// anything at all seeing it.
    unsafe extern "system" fn watch(code: i32, what: WPARAM, data: LPARAM) -> LRESULT {
        // Negative means Windows is asking to be left alone, and the documented answer is to pass
        // it straight on without looking at it.
        if code < 0 || !TAKING.load(Ordering::Relaxed) {
            return unsafe { CallNextHookEx(std::ptr::null_mut(), code, what, data) };
        }

        // SAFETY: for a keyboard hook with a non-negative code, Windows documents the pointer as a
        // `KBDLLHOOKSTRUCT` that is valid for the duration of this call.
        let key = unsafe { &*(data as *const KBDLLHOOKSTRUCT) };
        let pressed = key.vkCode;

        let spared = pressed == SPARE_ONE.load(Ordering::Relaxed)
            || pressed == SPARE_TWO.load(Ordering::Relaxed);

        // A modifier held is a shortcut rather than a letter, and a shortcut belongs to whatever
        // has focus. Alt-tab and control-anything keep working while a box is being typed into.
        let held = |key: u16| unsafe {
            windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(i32::from(key))
        } as u16
            & 0x8000
            != 0;

        // Control and alt are shortcuts for whatever has focus, and taking them would cost the
        // Commander alt-tab while a text box happened to be focused. The three clipboard ones are
        // the exception: they belong to the box being typed into, and letting them past would
        // paste into the game instead.
        let clipboard = matches!(pressed as u16, CUT | COPY | PASTE);
        let shortcut = held(VK_CONTROL) && !clipboard;

        if spared || shortcut || held(VK_MENU) {
            return unsafe { CallNextHookEx(std::ptr::null_mut(), code, what, data) };
        }

        // Taken. Nothing else on the machine sees this key.
        1
    }
}

/// The Windows clipboard, for the three shortcuts everybody expects to work in a text box.
///
/// Read and written directly, because the panel is not a window and nothing is going to do it on
/// its behalf. Every call opens the clipboard, does one thing and closes it: it is a system-wide
/// lock, and an application that holds it is one nothing else can copy or paste while it does.
pub mod clipboard {
    use windows_sys::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
    };

    /// Unicode text, which is the only format Ward reads or writes.
    const TEXT: u32 = 13;

    /// What is on the clipboard, if it is text.
    #[must_use]
    pub fn read() -> Option<String> {
        if unsafe { OpenClipboard(std::ptr::null_mut()) } == 0 {
            return None;
        }

        let handle: HANDLE = unsafe { GetClipboardData(TEXT) };

        let text = if handle.is_null() {
            None
        } else {
            let locked = unsafe { GlobalLock(handle as HGLOBAL) }.cast::<u16>();

            if locked.is_null() {
                None
            } else {
                // SAFETY: Windows guarantees a terminated wide string for `CF_UNICODETEXT`, and
                // the lock above keeps it in place until it is released below.
                let mut wide = Vec::new();
                let mut at = 0isize;

                loop {
                    let unit = unsafe { *locked.offset(at) };
                    if unit == 0 {
                        break;
                    }
                    wide.push(unit);
                    at += 1;
                }

                unsafe { GlobalUnlock(handle as HGLOBAL) };
                Some(String::from_utf16_lossy(&wide))
            }
        };

        unsafe { CloseClipboard() };
        text
    }

    /// Puts text on the clipboard.
    ///
    /// The memory handed over is not freed here. Windows takes ownership of it the moment
    /// `SetClipboardData` succeeds, and freeing it afterwards is a use-after-free in whatever
    /// pastes next.
    pub fn write(text: &str) {
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);

        if unsafe { OpenClipboard(std::ptr::null_mut()) } == 0 {
            return;
        }

        unsafe { EmptyClipboard() };

        let bytes = std::mem::size_of_val(wide.as_slice());
        let block = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };

        if !block.is_null() {
            let into = unsafe { GlobalLock(block) }.cast::<u16>();

            if into.is_null() {
                // Nothing owns the block yet, so it is ours to drop.
                unsafe { windows_sys::Win32::Foundation::GlobalFree(block) };
            } else {
                // SAFETY: the block was allocated at exactly this size a few lines above, and the
                // lock keeps it in place for the copy.
                unsafe {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), into, wide.len());
                    GlobalUnlock(block);
                }

                if unsafe { SetClipboardData(TEXT, block as HANDLE) }.is_null() {
                    // It was refused, so ownership never passed and the block is still ours.
                    unsafe { windows_sys::Win32::Foundation::GlobalFree(block) };
                }
            }
        }

        unsafe { CloseClipboard() };
    }
}
