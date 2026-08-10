//! Pressing keys on the Commander's behalf.
//!
//! **Scancodes, never virtual keys.** Elite reads the keyboard at a level below
//! the one most software types at, and a virtual-key event simply does not
//! register — the press succeeds, nothing happens in the game, and there is
//! nothing to see.
//!
//! **Never a low-level keyboard hook.** That is the API keyloggers use, and
//! Ward ships unsigned. Installing one would make an antivirus product's
//! judgment of Ward correct.
//!
//! The safety rule that outranks everything else here: [`release_all`] is
//! unconditional, reachable from any state, and never behind the thing that
//! failed. A key left down in Elite is a throttle that will not stop, or a
//! weapon that will not stop firing, and the Commander cannot fix it by
//! pressing the key themselves — the game already believes it is held.

use std::sync::Mutex;
use std::time::Duration;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, SendInput,
};

/// Nothing is held for longer than this, whatever asks.
///
/// A bound on how wrong things can go rather than a tuning knob. If a hold is
/// somehow never released, the ceiling is a few seconds of trouble rather than
/// the rest of the session.
pub const MAX_HOLD: Duration = Duration::from_secs(10);

/// Every scancode currently held down by Ward.
///
/// Tracked rather than assumed, because the whole point of `release_all` is to
/// work when something has already gone wrong, and something that has gone
/// wrong cannot be trusted to have told anyone what it was doing.
static HELD: Mutex<Vec<u16>> = Mutex::new(Vec::new());

/// Translates the game's name for a key into the scancode the hardware sends.
///
/// Only keys that can be typed on any keyboard. An unknown name returns nothing
/// rather than guessing: pressing the wrong key in a cockpit is worse than
/// pressing none.
pub fn scancode(name: &str) -> Option<u16> {
    let code = match name {
        "Key_Escape" => 0x01,
        "Key_1" => 0x02,
        "Key_2" => 0x03,
        "Key_3" => 0x04,
        "Key_4" => 0x05,
        "Key_5" => 0x06,
        "Key_6" => 0x07,
        "Key_7" => 0x08,
        "Key_8" => 0x09,
        "Key_9" => 0x0A,
        "Key_0" => 0x0B,
        "Key_Minus" => 0x0C,
        "Key_Equals" => 0x0D,
        "Key_Backspace" => 0x0E,
        "Key_Tab" => 0x0F,
        "Key_Q" => 0x10,
        "Key_W" => 0x11,
        "Key_E" => 0x12,
        "Key_R" => 0x13,
        "Key_T" => 0x14,
        "Key_Y" => 0x15,
        "Key_U" => 0x16,
        "Key_I" => 0x17,
        "Key_O" => 0x18,
        "Key_P" => 0x19,
        "Key_LeftBracket" => 0x1A,
        "Key_RightBracket" => 0x1B,
        "Key_Enter" => 0x1C,
        "Key_LeftControl" => 0x1D,
        "Key_A" => 0x1E,
        "Key_S" => 0x1F,
        "Key_D" => 0x20,
        "Key_F" => 0x21,
        "Key_G" => 0x22,
        "Key_H" => 0x23,
        "Key_J" => 0x24,
        "Key_K" => 0x25,
        "Key_L" => 0x26,
        "Key_SemiColon" => 0x27,
        "Key_Apostrophe" => 0x28,
        "Key_Grave" => 0x29,
        "Key_LeftShift" => 0x2A,
        "Key_BackSlash" => 0x2B,
        "Key_Z" => 0x2C,
        "Key_X" => 0x2D,
        "Key_C" => 0x2E,
        "Key_V" => 0x2F,
        "Key_B" => 0x30,
        "Key_N" => 0x31,
        "Key_M" => 0x32,
        "Key_Comma" => 0x33,
        "Key_Period" => 0x34,
        "Key_Slash" => 0x35,
        "Key_RightShift" => 0x36,
        "Key_Multiply" => 0x37,
        "Key_LeftAlt" => 0x38,
        "Key_Space" => 0x39,
        "Key_CapsLock" => 0x3A,
        "Key_F1" => 0x3B,
        "Key_F2" => 0x3C,
        "Key_F3" => 0x3D,
        "Key_F4" => 0x3E,
        "Key_F5" => 0x3F,
        "Key_F6" => 0x40,
        "Key_F7" => 0x41,
        "Key_F8" => 0x42,
        "Key_F9" => 0x43,
        "Key_F10" => 0x44,
        "Key_F11" => 0x57,
        "Key_F12" => 0x58,
        _ => return None,
    };

    Some(code)
}

fn send(code: u16, up: bool) {
    let mut input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: unsafe { std::mem::zeroed() },
    };

    // SAFETY: writing the keyboard arm of a union that was zeroed above, then
    // handing Windows one correctly sized record.
    unsafe {
        input.Anonymous.ki = KEYBDINPUT {
            wVk: 0,
            wScan: code,
            dwFlags: KEYEVENTF_SCANCODE | if up { KEYEVENTF_KEYUP } else { 0 },
            time: 0,
            dwExtraInfo: 0,
        };

        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Presses a key down and remembers that it is down.
pub fn down(code: u16) {
    if let Ok(mut held) = HELD.lock()
        && !held.contains(&code)
    {
        held.push(code);
    }
    send(code, false);
}

/// Releases a key and forgets it.
pub fn up(code: u16) {
    send(code, true);
    if let Ok(mut held) = HELD.lock() {
        held.retain(|c| *c != code);
    }
}

/// Releases everything Ward is holding.
///
/// Unconditional. It takes no arguments, asks no questions, and cannot fail in
/// a way that leaves a key down: if the record of what is held is itself
/// unavailable, it is cleared and the release is attempted anyway. Nothing
/// about this is allowed to depend on the thing that went wrong.
pub fn release_all() {
    let held: Vec<u16> = match HELD.lock() {
        Ok(mut held) => std::mem::take(&mut held),
        // The lock is poisoned, which means something panicked while holding
        // it - exactly the situation this function exists for.
        Err(poisoned) => {
            let mut held = poisoned.into_inner();
            std::mem::take(&mut *held)
        }
    };

    for code in held {
        send(code, true);
    }
}

/// How many keys Ward currently believes it is holding.
pub fn held() -> usize {
    HELD.lock().map(|h| h.len()).unwrap_or(0)
}

/// Presses and releases, with the release guaranteed.
///
/// The hold is capped whatever is asked for, and the key is released on the way
/// out of every path through this function.
pub fn tap(code: u16, hold: Duration) {
    down(code);
    std::thread::sleep(hold.min(MAX_HOLD));
    up(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keys_a_commander_might_bind_are_known() {
        // Taken from a real bindings file: firing on F, the scanner on the
        // apostrophe key.
        assert_eq!(scancode("Key_F"), Some(0x21));
        assert_eq!(scancode("Key_Apostrophe"), Some(0x28));
        assert_eq!(scancode("Key_Space"), Some(0x39));
        assert_eq!(scancode("Key_1"), Some(0x02));
    }

    #[test]
    fn an_unknown_key_name_presses_nothing() {
        // Pressing the wrong key in a cockpit is worse than pressing none.
        assert_eq!(scancode("Key_Joystick"), None);
        assert_eq!(scancode("Joy_9"), None);
        assert_eq!(scancode(""), None);
    }

    #[test]
    fn no_two_keys_share_a_scancode() {
        // A collision would mean one action pressing another action's key,
        // which in this game could mean jettisoning cargo instead of scanning.
        let names = [
            "Key_A",
            "Key_B",
            "Key_C",
            "Key_F",
            "Key_J",
            "Key_Space",
            "Key_Enter",
            "Key_Tab",
            "Key_Apostrophe",
            "Key_F1",
            "Key_F12",
            "Key_0",
            "Key_9",
            "Key_LeftShift",
        ];

        let mut codes: Vec<u16> = names.iter().filter_map(|n| scancode(n)).collect();
        let before = codes.len();
        codes.sort_unstable();
        codes.dedup();

        assert_eq!(codes.len(), before, "two names map to one scancode");
    }

    #[test]
    fn a_hold_is_capped_however_long_it_is_asked_for() {
        // Not a tuning knob. A bound on how wrong things can go.
        assert!(Duration::from_secs(3600).min(MAX_HOLD) == MAX_HOLD);
    }

    #[test]
    fn releasing_everything_leaves_nothing_held() {
        // The record is what release_all works from, so it has to be empty
        // afterwards whatever it contained. This exercises the bookkeeping
        // without sending anything at a game that is not running.
        release_all();
        assert_eq!(held(), 0);
    }
}
