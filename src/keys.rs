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
    pub fn typed(&mut self, listening: bool) -> String {
        if !listening {
            self.primed = false;
            return String::new();
        }

        let now = read_all();

        // The first pass after focus arrives records and says nothing. Everything held at that
        // moment was pressed for some other reason.
        if !self.primed {
            self.down = now;
            self.primed = true;
            return String::new();
        }

        let mut typed = String::new();

        for key in fresh(&now, &self.down) {
            typed.push_str(&character(key));
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

        assert_eq!(keyboard.typed(false), "");
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
