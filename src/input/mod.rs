//! Synthetic input emission (Linux `/dev/uinput`) and Unicode text injection.

pub mod inject;
pub mod scheduler;
#[cfg(target_os = "linux")]
pub mod uinput;

use std::fmt;

#[derive(Clone, Debug)]
pub enum InputError {
    PermissionDenied(String),
    UnsupportedKey(u16),
    Io(String),
    UnsupportedPlatform,
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PermissionDenied(msg) => write!(
                f,
                "cannot open /dev/uinput ({msg}); run `open-au05 setup udev` to grant access"
            ),
            Self::UnsupportedKey(code) => write!(f, "unsupported HID code 0x{code:04X}"),
            Self::Io(msg) => write!(f, "uinput I/O error: {msg}"),
            Self::UnsupportedPlatform => f.write_str("uinput is only supported on Linux"),
        }
    }
}

impl std::error::Error for InputError {}

pub trait InputEmitter: Send {
    fn ensure(&mut self) -> Result<(), InputError>;
    fn is_ready(&self) -> bool {
        true
    }
    fn emit_hid(&mut self, hid_code: u16, down: bool) -> Result<(), InputError>;
    fn emit_fixed_function(&mut self, function: u32, down: bool) -> Result<(), InputError>;
    fn close(&mut self);
}

/// Converts a USB HID usage code (keyboard 0x04..0xE7, mouse 0x100..0x102)
/// to a Linux input-event-codes `KEY_*` / `BTN_*` constant.
pub fn hid_to_linux_key(hid_code: u16) -> Option<u16> {
    // Letters A..Z (0x04..=0x1D)
    const LETTERS: [u16; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17,
        45, 21, 44,
    ];
    match hid_code {
        0x04..=0x1D => Some(LETTERS[(hid_code - 0x04) as usize]),
        0x1E..=0x26 => Some(hid_code - 0x1E + 2), // KEY_1..KEY_9 (2..10)
        0x27 => Some(11),                         // KEY_0
        0x28 => Some(28),                         // KEY_ENTER
        0x29 => Some(1),                          // KEY_ESC
        0x2A => Some(14),                         // KEY_BACKSPACE
        0x2B => Some(15),                         // KEY_TAB
        0x2C => Some(57),                         // KEY_SPACE
        0x2D => Some(12),                         // KEY_MINUS
        0x2E => Some(13),                         // KEY_EQUAL
        0x2F => Some(26),                         // KEY_LEFTBRACE
        0x30 => Some(27),                         // KEY_RIGHTBRACE
        0x31 => Some(43),                         // KEY_BACKSLASH
        0x33 => Some(39),                         // KEY_SEMICOLON
        0x34 => Some(40),                         // KEY_APOSTROPHE
        0x35 => Some(41),                         // KEY_GRAVE
        0x36 => Some(51),                         // KEY_COMMA
        0x37 => Some(52),                         // KEY_DOT
        0x38 => Some(53),                         // KEY_SLASH
        0x39 => Some(58),                         // KEY_CAPSLOCK
        0x3A..=0x43 => Some(hid_code - 0x3A + 59), // KEY_F1..KEY_F10 (59..68)
        0x44 => Some(87),                         // KEY_F11
        0x45 => Some(88),                         // KEY_F12
        0x46 => Some(99),                         // KEY_SYSRQ (PrintScreen)
        0x47 => Some(70),                         // KEY_SCROLLLOCK
        0x48 => Some(119),                        // KEY_PAUSE
        0x49 => Some(110),                        // KEY_INSERT
        0x4A => Some(102),                        // KEY_HOME
        0x4B => Some(104),                        // KEY_PAGEUP
        0x4C => Some(111),                        // KEY_DELETE
        0x4D => Some(107),                        // KEY_END
        0x4E => Some(109),                        // KEY_PAGEDOWN
        0x4F => Some(106),                        // KEY_RIGHT
        0x50 => Some(105),                        // KEY_LEFT
        0x51 => Some(108),                        // KEY_DOWN
        0x52 => Some(103),                        // KEY_UP
        0x53 => Some(69),                         // KEY_NUMLOCK
        0x54 => Some(98),                         // KEY_KPSLASH
        0x55 => Some(55),                         // KEY_KPASTERISK
        0x56 => Some(74),                         // KEY_KPMINUS
        0x57 => Some(78),                         // KEY_KPPLUS
        0x58 => Some(96),                         // KEY_KPENTER
        0x59 => Some(79),                         // KEY_KP1
        0x5A => Some(80),                         // KEY_KP2
        0x5B => Some(81),                         // KEY_KP3
        0x5C => Some(75),                         // KEY_KP4
        0x5D => Some(76),                         // KEY_KP5
        0x5E => Some(77),                         // KEY_KP6
        0x5F => Some(71),                         // KEY_KP7
        0x60 => Some(72),                         // KEY_KP8
        0x61 => Some(73),                         // KEY_KP9
        0x62 => Some(82),                         // KEY_KP0
        0x63 => Some(83),                         // KEY_KPDOT
        0x65 => Some(139),                        // KEY_MENU
        0x66 => Some(116),                        // KEY_POWER
        0x67 => Some(117),                        // KEY_KPEQUAL
        0x68..=0x73 => Some(hid_code - 0x68 + 183), // KEY_F13..KEY_F24 (183..194)
        0x75 => Some(138),                        // KEY_HELP
        0x7F => Some(113),                        // KEY_MUTE
        0x80 => Some(115),                        // KEY_VOLUMEUP
        0x81 => Some(114),                        // KEY_VOLUMEDOWN
        0xE0 => Some(29),                         // KEY_LEFTCTRL
        0xE1 => Some(42),                         // KEY_LEFTSHIFT
        0xE2 => Some(56),                         // KEY_LEFTALT
        0xE3 => Some(125),                        // KEY_LEFTMETA
        0xE4 => Some(97),                         // KEY_RIGHTCTRL
        0xE5 => Some(54),                         // KEY_RIGHTSHIFT
        0xE6 => Some(100),                        // KEY_RIGHTALT
        0xE7 => Some(126),                        // KEY_RIGHTMETA
        // Mouse buttons
        0x100 => Some(0x110), // BTN_LEFT
        0x101 => Some(0x111), // BTN_RIGHT
        0x102 => Some(0x112), // BTN_MIDDLE
        _ => None,
    }
}

/// Converts an AU05 fixed function code (media keys) to a Linux `KEY_*` code.
pub fn fixed_function_to_linux_key(code: u32) -> Option<u16> {
    match code {
        7 => Some(164),  // KEY_PLAYPAUSE
        8 => Some(163),  // KEY_NEXTSONG
        9 => Some(165),  // KEY_PREVIOUSSONG
        11 => Some(113), // KEY_MUTE
        12 => Some(115), // KEY_VOLUMEUP
        13 => Some(114), // KEY_VOLUMEDOWN
        _ => None,
    }
}

/// No-op mock emitter for unit tests and dry-runs.
#[derive(Default)]
pub struct MockInputEmitter {
    pub events: Vec<(u16, bool)>,
    pub fixed_events: Vec<(u32, bool)>,
}

impl InputEmitter for MockInputEmitter {
    fn ensure(&mut self) -> Result<(), InputError> {
        Ok(())
    }

    fn emit_hid(&mut self, hid_code: u16, down: bool) -> Result<(), InputError> {
        self.events.push((hid_code, down));
        Ok(())
    }

    fn emit_fixed_function(&mut self, function: u32, down: bool) -> Result<(), InputError> {
        self.fixed_events.push((function, down));
        Ok(())
    }

    fn close(&mut self) {}
}

pub fn create_default_emitter() -> Box<dyn InputEmitter> {
    #[cfg(target_os = "linux")]
    {
        Box::new(uinput::UInputEmitter::default())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Box::new(MockInputEmitter::default())
    }
}
