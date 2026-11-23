//! Hotkey parsing, rendering, and firmware shortcut/fixed-function encoding.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// One `(page, value)` pair stored in AU05 onboard flash for a button shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutKey {
    pub page: u8,
    pub value: u8,
    pub sign: bool,
}

impl ShortcutKey {
    pub fn new(page: u8, value: u8, sign: bool) -> Self {
        Self { page, value, sign }
    }

    /// Encodes a 16-bit host HID code using the Ulanzi Studio mapping:
    /// - `0xE0..=0xE7` -> page 3, value `1 << (code - 0xE0)` (modifier bitmask)
    /// - `0x100..=0x106` -> page 7, value `code & 0xFF` (mouse button/wheel)
    /// - `0x02` -> page `0x13`, value `2` (Fn / Globe)
    /// - anything else -> page 2, value `code as u8` (keyboard usage)
    pub fn from_hid_code(code: u16) -> Self {
        match code {
            0xE0..=0xE7 => Self::new(3, 1u8 << (code - 0xE0), false),
            0x100..=0x106 => Self::new(7, (code & 0xFF) as u8, false),
            0x02 => Self::new(0x13, 2, false),
            other => Self::new(2, other as u8, false),
        }
    }

    pub fn hid_code(self) -> u16 {
        match self.page {
            3 => {
                for bit in 0..8u8 {
                    if self.value == (1u8 << bit) {
                        return 0xE0 + u16::from(bit);
                    }
                }
                0xE0
            }
            7 => 0x100 | u16::from(self.value),
            0x13 => 0x02,
            _ => u16::from(self.value),
        }
    }
}

impl fmt::Display for ShortcutKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(name) = Hotkey::name_for_hid(self.hid_code()) {
            f.write_str(name)
        } else {
            write!(f, "page{:02x}:{:02x}", self.page, self.value)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyError {
    Empty,
    UnknownKey(String),
    TooManyKeys(usize),
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("hotkey is empty"),
            Self::UnknownKey(k) => {
                write!(
                    f,
                    "unknown key name '{k}' (run `open-au05 keys` to list valid names)"
                )
            }
            Self::TooManyKeys(n) => {
                write!(f, "a button holds at most 4 keys, got {n}")
            }
        }
    }
}

impl std::error::Error for HotkeyError {}

pub struct Hotkey;

impl Hotkey {
    /// Parses a hotkey like `ctrl+shift+v`, `super+space`, `wheel-up`, or `fn`
    /// into firmware `ShortcutKey` pairs (modifiers first, max 4 keys).
    pub fn parse(text: &str) -> Result<Vec<ShortcutKey>, HotkeyError> {
        let codes = Self::parse_hid_codes(text)?;
        Ok(codes.into_iter().map(ShortcutKey::from_hid_code).collect())
    }

    pub fn parse_hid_codes(text: &str) -> Result<Vec<u16>, HotkeyError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(HotkeyError::Empty);
        }
        let mut parts: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut prev: Option<char> = None;
        for ch in trimmed.chars() {
            if ch == '+' && !current.is_empty() && prev != Some('+') {
                parts.push(std::mem::take(&mut current));
            } else {
                current.push(ch);
            }
            prev = Some(ch);
        }
        if !current.is_empty() {
            parts.push(current);
        }

        let mut modifiers = Vec::new();
        let mut keys = Vec::new();
        for raw in parts {
            let name = raw.trim().to_ascii_lowercase();
            let code = Self::code_for_name(&name).ok_or(HotkeyError::UnknownKey(raw))?;
            if (0xE0..=0xE7).contains(&code) {
                if !modifiers.contains(&code) {
                    modifiers.push(code);
                }
            } else {
                keys.push(code);
            }
        }
        let mut all = modifiers;
        all.extend(keys);
        if all.is_empty() {
            return Err(HotkeyError::Empty);
        }
        if all.len() > 4 {
            return Err(HotkeyError::TooManyKeys(all.len()));
        }
        Ok(all)
    }

    pub fn code_for_name(name: &str) -> Option<u16> {
        let n = name.trim().to_ascii_lowercase();
        if let Some(c) = alias_code(&n) {
            return Some(c);
        }
        if n.len() == 1 {
            let b = n.as_bytes()[0];
            if b.is_ascii_lowercase() {
                return Some(0x04 + u16::from(b - b'a'));
            }
            if (b'1'..=b'9').contains(&b) {
                return Some(0x1E + u16::from(b - b'1'));
            }
            if b == b'0' {
                return Some(0x27);
            }
            return match b {
                b'-' => Some(0x2D),
                b'=' | b'+' => Some(0x2E),
                b'[' => Some(0x2F),
                b']' => Some(0x30),
                b'\\' => Some(0x31),
                b';' => Some(0x33),
                b'\'' => Some(0x34),
                b'`' => Some(0x35),
                b',' => Some(0x36),
                b'.' => Some(0x37),
                b'/' => Some(0x38),
                _ => None,
            };
        }
        if let Some(rest) = n.strip_prefix('f') {
            if let Ok(num) = rest.parse::<u16>() {
                if (1..=12).contains(&num) {
                    return Some(0x3A + (num - 1));
                }
                if (13..=24).contains(&num) {
                    return Some(0x68 + (num - 13));
                }
            }
        }
        if let Some(hex) = n.strip_prefix("0x") {
            return u16::from_str_radix(hex, 16).ok();
        }
        None
    }

    pub fn name_for_hid(code: u16) -> Option<&'static str> {
        canonical_name(code)
    }

    pub fn render(keys: &[ShortcutKey]) -> String {
        let mut parts = Vec::new();
        for k in keys {
            if k.page == 3 {
                for bit in 0..8u8 {
                    if k.value & (1u8 << bit) != 0 {
                        let code = 0xE0 + u16::from(bit);
                        parts.push(
                            canonical_name(code)
                                .map(String::from)
                                .unwrap_or_else(|| format!("mod{bit}")),
                        );
                    }
                }
            } else {
                parts.push(k.to_string());
            }
        }
        parts.join("+")
    }

    /// Returns all canonical HID codes with their accepted names for `open-au05 keys`.
    pub fn catalog() -> Vec<(u16, Vec<&'static str>)> {
        let mut map: BTreeMap<u16, Vec<&'static str>> = BTreeMap::new();
        for &(name, code) in ALIASES {
            map.entry(code).or_default().push(name);
        }
        for code in 0x04..=0x27 {
            if let Some(name) = canonical_name(code) {
                map.entry(code).or_default().push(name);
            }
        }
        for code in 0x3A..=0x45 {
            if let Some(name) = canonical_name(code) {
                map.entry(code).or_default().push(name);
            }
        }
        for code in 0x68..=0x73 {
            if let Some(name) = canonical_name(code) {
                map.entry(code).or_default().push(name);
            }
        }
        map.into_iter()
            .map(|(code, mut names)| {
                names.sort_unstable();
                names.dedup();
                if let Some(canon) = canonical_name(code) {
                    if let Some(pos) = names.iter().position(|&n| n == canon) {
                        names.remove(pos);
                    }
                    names.insert(0, canon);
                }
                (code, names)
            })
            .collect()
    }
}

const ALIASES: &[(&str, u16)] = &[
    // Modifiers
    ("ctrl", 0xE0),
    ("control", 0xE0),
    ("lctrl", 0xE0),
    ("shift", 0xE1),
    ("lshift", 0xE1),
    ("alt", 0xE2),
    ("option", 0xE2),
    ("opt", 0xE2),
    ("lalt", 0xE2),
    ("super", 0xE3),
    ("meta", 0xE3),
    ("cmd", 0xE3),
    ("command", 0xE3),
    ("gui", 0xE3),
    ("win", 0xE3),
    ("lsuper", 0xE3),
    ("rctrl", 0xE4),
    ("rcontrol", 0xE4),
    ("rshift", 0xE5),
    ("ralt", 0xE6),
    ("altgr", 0xE6),
    ("rsuper", 0xE7),
    ("rcmd", 0xE7),
    // Fn / Globe
    ("fn", 0x02),
    ("globe", 0x02),
    // Editing & navigation
    ("return", 0x28),
    ("enter", 0x28),
    ("escape", 0x29),
    ("esc", 0x29),
    ("backspace", 0x2A),
    ("tab", 0x2B),
    ("space", 0x2C),
    ("spacebar", 0x2C),
    ("minus", 0x2D),
    ("equal", 0x2E),
    ("equals", 0x2E),
    ("plus", 0x2E),
    ("leftbracket", 0x2F),
    ("rightbracket", 0x30),
    ("backslash", 0x31),
    ("semicolon", 0x33),
    ("quote", 0x34),
    ("apostrophe", 0x34),
    ("grave", 0x35),
    ("backtick", 0x35),
    ("comma", 0x36),
    ("period", 0x37),
    ("dot", 0x37),
    ("slash", 0x38),
    ("capslock", 0x39),
    ("caps", 0x39),
    ("printscreen", 0x46),
    ("scrolllock", 0x47),
    ("pause", 0x48),
    ("insert", 0x49),
    ("ins", 0x49),
    ("home", 0x4A),
    ("pageup", 0x4B),
    ("pgup", 0x4B),
    ("delete", 0x4C),
    ("del", 0x4C),
    ("forwarddelete", 0x4C),
    ("end", 0x4D),
    ("pagedown", 0x4E),
    ("pgdn", 0x4E),
    ("right", 0x4F),
    ("rightarrow", 0x4F),
    ("left", 0x50),
    ("leftarrow", 0x50),
    ("down", 0x51),
    ("downarrow", 0x51),
    ("up", 0x52),
    ("uparrow", 0x52),
    ("numlock", 0x53),
    ("kp/", 0x54),
    ("kp*", 0x55),
    ("kp-", 0x56),
    ("kp+", 0x57),
    ("kpenter", 0x58),
    ("kp1", 0x59),
    ("kp2", 0x5A),
    ("kp3", 0x5B),
    ("kp4", 0x5C),
    ("kp5", 0x5D),
    ("kp6", 0x5E),
    ("kp7", 0x5F),
    ("kp8", 0x60),
    ("kp9", 0x61),
    ("kp0", 0x62),
    ("kp.", 0x63),
    ("menu", 0x65),
    ("application", 0x65),
    ("power", 0x66),
    ("kp=", 0x67),
    ("help", 0x75),
    ("mute", 0x7F),
    ("volumeup", 0x80),
    ("volumedown", 0x81),
    // Mouse & wheel actions
    ("left-click", 0x100),
    ("leftclick", 0x100),
    ("click", 0x100),
    ("mouse1", 0x100),
    ("right-click", 0x101),
    ("rightclick", 0x101),
    ("mouse2", 0x101),
    ("middle-click", 0x102),
    ("middleclick", 0x102),
    ("mouse3", 0x102),
    ("wheel-up", 0x105),
    ("wheelup", 0x105),
    ("scroll-up", 0x105),
    ("scrollup", 0x105),
    ("wheel-down", 0x106),
    ("wheeldown", 0x106),
    ("scroll-down", 0x106),
    ("scrolldown", 0x106),
];

fn alias_code(name: &str) -> Option<u16> {
    ALIASES.iter().find(|&&(n, _)| n == name).map(|&(_, c)| c)
}

fn canonical_name(code: u16) -> Option<&'static str> {
    match code {
        0x02 => Some("fn"),
        0x04 => Some("a"),
        0x05 => Some("b"),
        0x06 => Some("c"),
        0x07 => Some("d"),
        0x08 => Some("e"),
        0x09 => Some("f"),
        0x0A => Some("g"),
        0x0B => Some("h"),
        0x0C => Some("i"),
        0x0D => Some("j"),
        0x0E => Some("k"),
        0x0F => Some("l"),
        0x10 => Some("m"),
        0x11 => Some("n"),
        0x12 => Some("o"),
        0x13 => Some("p"),
        0x14 => Some("q"),
        0x15 => Some("r"),
        0x16 => Some("s"),
        0x17 => Some("t"),
        0x18 => Some("u"),
        0x19 => Some("v"),
        0x1A => Some("w"),
        0x1B => Some("x"),
        0x1C => Some("y"),
        0x1D => Some("z"),
        0x1E => Some("1"),
        0x1F => Some("2"),
        0x20 => Some("3"),
        0x21 => Some("4"),
        0x22 => Some("5"),
        0x23 => Some("6"),
        0x24 => Some("7"),
        0x25 => Some("8"),
        0x26 => Some("9"),
        0x27 => Some("0"),
        0x28 => Some("return"),
        0x29 => Some("escape"),
        0x2A => Some("backspace"),
        0x2B => Some("tab"),
        0x2C => Some("space"),
        0x2D => Some("-"),
        0x2E => Some("="),
        0x2F => Some("["),
        0x30 => Some("]"),
        0x31 => Some("\\"),
        0x33 => Some(";"),
        0x34 => Some("'"),
        0x35 => Some("`"),
        0x36 => Some(","),
        0x37 => Some("."),
        0x38 => Some("/"),
        0x39 => Some("capslock"),
        0x3A => Some("f1"),
        0x3B => Some("f2"),
        0x3C => Some("f3"),
        0x3D => Some("f4"),
        0x3E => Some("f5"),
        0x3F => Some("f6"),
        0x40 => Some("f7"),
        0x41 => Some("f8"),
        0x42 => Some("f9"),
        0x43 => Some("f10"),
        0x44 => Some("f11"),
        0x45 => Some("f12"),
        0x46 => Some("printscreen"),
        0x47 => Some("scrolllock"),
        0x48 => Some("pause"),
        0x49 => Some("insert"),
        0x4A => Some("home"),
        0x4B => Some("pageup"),
        0x4C => Some("delete"),
        0x4D => Some("end"),
        0x4E => Some("pagedown"),
        0x4F => Some("right"),
        0x50 => Some("left"),
        0x51 => Some("down"),
        0x52 => Some("up"),
        0x53 => Some("numlock"),
        0x54 => Some("kp/"),
        0x55 => Some("kp*"),
        0x56 => Some("kp-"),
        0x57 => Some("kp+"),
        0x58 => Some("kpenter"),
        0x59 => Some("kp1"),
        0x5A => Some("kp2"),
        0x5B => Some("kp3"),
        0x5C => Some("kp4"),
        0x5D => Some("kp5"),
        0x5E => Some("kp6"),
        0x5F => Some("kp7"),
        0x60 => Some("kp8"),
        0x61 => Some("kp9"),
        0x62 => Some("kp0"),
        0x63 => Some("kp."),
        0x65 => Some("menu"),
        0x66 => Some("power"),
        0x67 => Some("kp="),
        0x68 => Some("f13"),
        0x69 => Some("f14"),
        0x6A => Some("f15"),
        0x6B => Some("f16"),
        0x6C => Some("f17"),
        0x6D => Some("f18"),
        0x6E => Some("f19"),
        0x6F => Some("f20"),
        0x70 => Some("f21"),
        0x71 => Some("f22"),
        0x72 => Some("f23"),
        0x73 => Some("f24"),
        0x75 => Some("help"),
        0x7F => Some("mute"),
        0x80 => Some("volumeup"),
        0x81 => Some("volumedown"),
        0xE0 => Some("ctrl"),
        0xE1 => Some("shift"),
        0xE2 => Some("alt"),
        0xE3 => Some("super"),
        0xE4 => Some("rctrl"),
        0xE5 => Some("rshift"),
        0xE6 => Some("ralt"),
        0xE7 => Some("rsuper"),
        0x100 => Some("left-click"),
        0x101 => Some("right-click"),
        0x102 => Some("middle-click"),
        0x105 => Some("wheel-up"),
        0x106 => Some("wheel-down"),
        _ => None,
    }
}

/// Firmware fixed functions (media keys and factory defaults).
pub struct FixedFunction;

impl FixedFunction {
    pub const NAMED: &'static [(&'static str, u32)] = &[
        ("play-pause", 7),
        ("next-track", 8),
        ("previous-track", 9),
        ("mute", 11),
        ("volume-up", 12),
        ("volume-down", 13),
        ("factory-knob-press", 0x6E),
        ("factory-talk", 0x6F),
        ("factory-confirm", 0x70),
        ("factory-cancel", 0x71),
        ("factory-knob-right", 0x72),
        ("factory-knob-left", 0x73),
    ];

    pub fn code_for_name(name: &str) -> Option<u32> {
        let n = name.trim().to_ascii_lowercase();
        if let Some(&(_, code)) = Self::NAMED.iter().find(|&&(k, _)| k == n) {
            return Some(code);
        }
        if let Some(hex) = n.strip_prefix("0x") {
            return u32::from_str_radix(hex, 16).ok();
        }
        n.parse::<u32>().ok()
    }

    pub fn name_for_code(code: u32) -> String {
        Self::NAMED
            .iter()
            .find(|&&(_, c)| c == code)
            .map(|&(n, _)| n.to_string())
            .unwrap_or_else(|| format!("0x{code:02X}"))
    }
}
