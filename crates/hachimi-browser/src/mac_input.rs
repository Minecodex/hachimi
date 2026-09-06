//! macOS `NSEvent` → Chromium/CEF input constants.
//!
//! Pure data tables shared by the desktop overlay (which translates AppKit
//! events into [`crate::CefInputEvent`] payloads) and unit tests. Mirrors
//! Chromium's `keyboard_code_conversion_mac.mm` for the keys the embedded
//! browser supports.

use crate::cef_event_flags;

/// Returns the Windows/Chromium virtual-key code for a macOS virtual key
/// code (`NSEvent.keyCode`), or 0 when the key is unmapped.
#[must_use]
#[allow(clippy::match_same_arms)]
pub fn mac_key_code_to_windows_vk(key_code: u16) -> i32 {
    // Windows virtual-key codes used below.
    const VK_BACK: i32 = 0x08;
    const VK_TAB: i32 = 0x09;
    const VK_CLEAR: i32 = 0x0C;
    const VK_RETURN: i32 = 0x0D;
    const VK_SHIFT: i32 = 0x10;
    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12;
    const VK_CAPITAL: i32 = 0x14;
    const VK_ESCAPE: i32 = 0x1B;
    const VK_SPACE: i32 = 0x20;
    const VK_PRIOR: i32 = 0x21; // Page Up
    const VK_NEXT: i32 = 0x22; // Page Down
    const VK_END: i32 = 0x23;
    const VK_HOME: i32 = 0x24;
    const VK_LEFT: i32 = 0x25;
    const VK_UP: i32 = 0x26;
    const VK_RIGHT: i32 = 0x27;
    const VK_DOWN: i32 = 0x28;
    const VK_INSERT: i32 = 0x2D;
    const VK_DELETE: i32 = 0x2E;
    const VK_LWIN: i32 = 0x5B;
    const VK_RSHIFT: i32 = 0xA1;
    const VK_RMENU: i32 = 0xA5;
    const VK_RCONTROL: i32 = 0xA3;
    const VK_OEM_1: i32 = 0xBA; // ';:'
    const VK_OEM_PLUS: i32 = 0xBB;
    const VK_OEM_COMMA: i32 = 0xBC;
    const VK_OEM_MINUS: i32 = 0xBD;
    const VK_OEM_PERIOD: i32 = 0xBE;
    const VK_OEM_2: i32 = 0xBF; // '/?'
    const VK_OEM_3: i32 = 0xC0; // '`~'
    const VK_OEM_4: i32 = 0xDB; // '[{'
    const VK_OEM_5: i32 = 0xDC; // '\|'
    const VK_OEM_6: i32 = 0xDD; // ']}'
    const VK_OEM_7: i32 = 0xDE; // quotes
    match key_code {
        // ANSI letters.
        0x00 => 0x41, // A
        0x01 => 0x53, // S
        0x02 => 0x44, // D
        0x03 => 0x46, // F
        0x04 => 0x48, // H
        0x05 => 0x47, // G
        0x06 => 0x5A, // Z
        0x07 => 0x58, // X
        0x08 => 0x43, // C
        0x09 => 0x56, // V
        0x0B => 0x42, // B
        0x0C => 0x51, // Q
        0x0D => 0x57, // W
        0x0E => 0x45, // E
        0x0F => 0x52, // R
        0x10 => 0x59, // Y
        0x11 => 0x54, // T
        0x1F => 0x4F, // O
        0x20 => 0x55, // U
        0x22 => 0x49, // I
        0x23 => 0x50, // P
        0x25 => 0x4C, // L
        0x26 => 0x4A, // J
        0x28 => 0x4B, // K
        0x2D => 0x4E, // N
        0x2E => 0x4D, // M
        // ANSI digit row.
        0x12 => 0x31,
        0x13 => 0x32,
        0x14 => 0x33,
        0x15 => 0x34,
        0x16 => 0x36,
        0x17 => 0x35,
        0x19 => 0x39,
        0x1A => 0x37,
        0x1C => 0x38,
        0x1D => 0x30,
        // Punctuation.
        0x18 => VK_OEM_PLUS,
        0x1B => VK_OEM_MINUS,
        0x1E => VK_OEM_6,
        0x21 => VK_OEM_4,
        0x27 => VK_OEM_7,
        0x29 => VK_OEM_1,
        0x2A => VK_OEM_5,
        0x2B => VK_OEM_COMMA,
        0x2C => VK_OEM_2,
        0x2F => VK_OEM_PERIOD,
        0x32 => VK_OEM_3,
        // Whitespace / control keys.
        0x24 | 0x4C => VK_RETURN, // Return / keypad Enter
        0x30 => VK_TAB,
        0x31 => VK_SPACE,
        0x33 => VK_BACK,
        0x35 => VK_ESCAPE,
        // Modifier keys.
        0x37 => VK_LWIN, // Command
        0x38 => VK_SHIFT,
        0x39 => VK_CAPITAL,
        0x3A => VK_MENU, // Option
        0x3B => VK_CONTROL,
        0x3C => VK_RSHIFT,
        0x3D => VK_RMENU, // Right Option
        0x3E => VK_RCONTROL,
        // Keypad.
        0x41 => 0x6E, // Decimal
        0x43 => 0x6A, // Multiply
        0x45 => 0x6B, // Add
        0x47 => VK_CLEAR,
        0x4B => 0x6F,        // Divide
        0x4E => 0x6D,        // Subtract
        0x51 => VK_OEM_PLUS, // keypad Equals
        0x52 => 0x60,        // keypad 0
        0x53 => 0x61,
        0x54 => 0x62,
        0x55 => 0x63,
        0x56 => 0x64,
        0x57 => 0x65,
        0x58 => 0x66,
        0x59 => 0x67,
        0x5B => 0x68,
        0x5C => 0x69, // keypad 9
        // Function keys.
        0x7A => 0x70, // F1
        0x78 => 0x71,
        0x63 => 0x72,
        0x76 => 0x73,
        0x60 => 0x74,
        0x61 => 0x75,
        0x62 => 0x76,
        0x64 => 0x77,
        0x65 => 0x78,
        0x6D => 0x79,
        0x67 => 0x7A,
        0x6F => 0x7B, // F12
        // Navigation cluster.
        0x72 => VK_INSERT, // Help / Insert
        0x73 => VK_HOME,
        0x74 => VK_PRIOR,
        0x75 => VK_DELETE, // Forward Delete
        0x77 => VK_END,
        0x79 => VK_NEXT,
        0x7B => VK_LEFT,
        0x7C => VK_RIGHT,
        0x7D => VK_DOWN,
        0x7E => VK_UP,
        _ => 0,
    }
}

/// Maps raw `NSEventModifierFlags` bits to the cef modifier bits defined in
/// [`cef_event_flags`].
#[must_use]
pub fn mac_event_modifiers_to_cef(raw: u64) -> u32 {
    const CAPS_LOCK: u64 = 1 << 16;
    const SHIFT: u64 = 1 << 17;
    const CONTROL: u64 = 1 << 18;
    const OPTION: u64 = 1 << 19;
    const COMMAND: u64 = 1 << 20;
    let mut modifiers = 0;
    if raw & CAPS_LOCK != 0 {
        modifiers |= cef_event_flags::CAPS_LOCK_ON;
    }
    if raw & SHIFT != 0 {
        modifiers |= cef_event_flags::SHIFT_DOWN;
    }
    if raw & CONTROL != 0 {
        modifiers |= cef_event_flags::CONTROL_DOWN;
    }
    if raw & OPTION != 0 {
        modifiers |= cef_event_flags::ALT_DOWN;
    }
    if raw & COMMAND != 0 {
        modifiers |= cef_event_flags::COMMAND_DOWN;
    }
    modifiers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_letters_and_digits_map_to_ascii_virtual_keys() {
        assert_eq!(mac_key_code_to_windows_vk(0x00), 0x41); // A
        assert_eq!(mac_key_code_to_windows_vk(0x0D), 0x57); // W
        assert_eq!(mac_key_code_to_windows_vk(0x12), 0x31); // 1
        assert_eq!(mac_key_code_to_windows_vk(0x1D), 0x30); // 0
        assert_eq!(mac_key_code_to_windows_vk(0xFFFF), 0);
    }

    #[test]
    fn modifier_flags_map_to_cef_bits() {
        assert_eq!(mac_event_modifiers_to_cef(0), 0);
        assert_eq!(
            mac_event_modifiers_to_cef((1 << 17) | (1 << 20)),
            cef_event_flags::SHIFT_DOWN | cef_event_flags::COMMAND_DOWN
        );
        assert_eq!(
            mac_event_modifiers_to_cef((1 << 18) | (1 << 19)),
            cef_event_flags::CONTROL_DOWN | cef_event_flags::ALT_DOWN
        );
    }
}
