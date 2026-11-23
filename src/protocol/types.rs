//! Core protocol constants and data structures for the Ulanzi Vibe Key AU05.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

pub const USB_VID: u16 = 0xFFF1;
pub const USB_PID: u16 = 0x00DD;
pub const USAGE_PAGE: u16 = 0xFFFC;
pub const REPORT_ID: u8 = 0x55;
pub const REPORT_LENGTH: usize = 64;

/// 16-entry XOR challenge-response authentication table recovered from Ulanzi Studio.
pub const AUTH_PRIVATE_KEYS: [u32; 16] = [
    0x2014_1107,
    0xC138_BF6D,
    0xA8AB_23C7,
    0x7316_5329,
    0x3062_9139,
    0xD9F4_E04C,
    0xEC8C_35BF,
    0x6410_1856,
    0x4720_1988,
    0x9821_1053,
    0xF0D4_ECA9,
    0x8299_1136,
    0x0915_3583,
    0x1614_1925,
    0xBF8A_7CED,
    0x5455_7049,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum MessageType {
    Device = 0x01,
    Dongle = 0x06,
    Notice = 0x0B,
    BleAudioC = 0x0C,
    BleAudioD = 0x0D,
    UsbHidAudio = 0x0E,
    UploadImage = 0x15,
    DongleUpgrade = 0x1E,
    DeviceUpgrade = 0x1F,
}

impl MessageType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v & 0x1F {
            0x01 => Some(Self::Device),
            0x06 => Some(Self::Dongle),
            0x0B => Some(Self::Notice),
            0x0C => Some(Self::BleAudioC),
            0x0D => Some(Self::BleAudioD),
            0x0E => Some(Self::UsbHidAudio),
            0x15 => Some(Self::UploadImage),
            0x1E => Some(Self::DongleUpgrade),
            0x1F => Some(Self::DeviceUpgrade),
            _ => None,
        }
    }
}

pub struct Operation;

impl Operation {
    pub const WRITE0: u8 = 0x00;
    pub const READ: u8 = 0x01;
    pub const WRITE2: u8 = 0x02;
    pub const WRITE4: u8 = 0x04;
    pub const REPLY_FLAG: u8 = 0x10;
    pub const SUCCESS_FLAG: u8 = 0x01;
}

/// The six physical controls on the Ulanzi Vibe Key AU05.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum Control {
    Talk = 0,
    Confirm = 1,
    Cancel = 2,
    KnobPress = 3,
    KnobRight = 4,
    KnobLeft = 5,
}

impl Control {
    pub const ALL: [Control; 6] = [
        Control::Talk,
        Control::Confirm,
        Control::Cancel,
        Control::KnobPress,
        Control::KnobRight,
        Control::KnobLeft,
    ];

    pub fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Talk),
            1 => Some(Self::Confirm),
            2 => Some(Self::Cancel),
            3 => Some(Self::KnobPress),
            4 => Some(Self::KnobRight),
            5 => Some(Self::KnobLeft),
            _ => None,
        }
    }

    pub fn index(self) -> u8 {
        self as u8
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Talk => "talk",
            Self::Confirm => "confirm",
            Self::Cancel => "cancel",
            Self::KnobPress => "knob-press",
            Self::KnobRight => "knob-right",
            Self::KnobLeft => "knob-left",
        }
    }

    pub fn is_rotation(self) -> bool {
        matches!(self, Self::KnobLeft | Self::KnobRight)
    }

    pub fn factory_fixed_function(self) -> u32 {
        match self {
            Self::Talk => 0x6F,
            Self::Confirm => 0x70,
            Self::Cancel => 0x71,
            Self::KnobPress => 0x6E,
            Self::KnobRight => 0x72,
            Self::KnobLeft => 0x73,
        }
    }
}

impl fmt::Display for Control {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Control {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "talk" | "k1" | "mic" | "voice" | "0" => Ok(Self::Talk),
            "confirm" | "k2" | "ok" | "enter" | "1" => Ok(Self::Confirm),
            "cancel" | "k3" | "esc" | "2" => Ok(Self::Cancel),
            "knob-press" | "knob_press" | "knobpress" | "knob" | "click" | "3" => {
                Ok(Self::KnobPress)
            }
            "knob-right" | "knob_right" | "knobright" | "cw" | "right" | "4" => Ok(Self::KnobRight),
            "knob-left" | "knob_left" | "knobleft" | "ccw" | "left" | "5" => Ok(Self::KnobLeft),
            other => Err(format!(
                "unknown control '{other}' (expected: talk, confirm, cancel, knob-press, knob-left, knob-right)"
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub custom_code: String,
    pub version: String,
    pub build_stamp: String,
    pub raw: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryStatus {
    pub percent: u16,
    pub millivolts: u16,
    pub charging: bool,
    pub charge_full: bool,
    pub low_battery_level: u8,
    pub power_off: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum LightMode {
    Off = 0,
    On = 1,
    Work = 2,
}

impl LightMode {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Off),
            1 => Some(Self::On),
            2 => Some(Self::Work),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
            Self::Work => "work",
        }
    }
}

impl FromStr for LightMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "0" | "off" => Ok(Self::Off),
            "1" | "on" | "all" | "all-on" => Ok(Self::On),
            "2" | "work" | "work-mode" | "agent" => Ok(Self::Work),
            other => Err(format!(
                "invalid light mode '{other}' (expected: off, on, work)"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum LedWorkType {
    Off = 0,
    Solid = 1,
    Breathe = 2,
}

impl LedWorkType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Off),
            1 => Some(Self::Solid),
            2 => Some(Self::Breathe),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Solid => "solid",
            Self::Breathe => "breathe",
        }
    }
}

impl FromStr for LedWorkType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "0" | "off" => Ok(Self::Off),
            "1" | "solid" | "on" => Ok(Self::Solid),
            "2" | "breathe" | "breathing" | "pulse" => Ok(Self::Breathe),
            other => Err(format!(
                "invalid LED effect '{other}' (expected: off, solid, breathe)"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LedField {
    WorkType = 0,
    WorkTime = 1,
    BreatheLevel = 2,
    BreatheBrightness = 3,
    AlwaysOnBrightness = 4,
}

impl LedField {
    pub fn mask(self) -> u8 {
        0x04 << (self as u8)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::WorkType => "type",
            Self::WorkTime => "time",
            Self::BreatheLevel => "breathe-level",
            Self::BreatheBrightness => "breathe-brightness",
            Self::AlwaysOnBrightness => "brightness",
        }
    }
}

impl FromStr for LedField {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "type" | "work-type" | "effect" => Ok(Self::WorkType),
            "time" | "work-time" => Ok(Self::WorkTime),
            "breathe-level" | "breathe" | "speed" => Ok(Self::BreatheLevel),
            "breathe-brightness" => Ok(Self::BreatheBrightness),
            "brightness" | "always-on-brightness" => Ok(Self::AlwaysOnBrightness),
            other => Err(format!(
                "unknown LED field '{other}' (expected: type, time, breathe-level, breathe-brightness, brightness)"
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedState {
    pub work_type: u8,
    pub work_time: u8,
    pub breathe_level: u8,
    pub breathe_brightness: u8,
    pub always_on_brightness: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndicatorLights {
    pub mode: u8,
    pub all_on_brightness: u8,
    pub leds: [LedState; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEvent {
    pub index: u8,
    pub status: u8,
    pub physical_index: u8,
}

impl KeyEvent {
    pub fn control(self) -> Option<Control> {
        Control::from_index(self.physical_index)
    }

    /// Status 0 = release; any non-zero status = press.
    /// Knob rotations are always press pulses.
    pub fn is_press(self) -> bool {
        if self.control().is_some_and(Control::is_rotation) {
            true
        } else {
            self.status != 0
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub hid_path: Option<String>,
    pub dongle_version: Option<VersionInfo>,
    pub dongle_serial: Option<String>,
    pub dongle_flash_id: Option<String>,
    pub dongle_charging: Option<u8>,
    pub mic_linked: bool,
    pub device_version: Option<VersionInfo>,
    pub hardware_version: Option<u8>,
    pub device_serial: Option<String>,
    pub device_uuid: Option<String>,
    pub device_flash_id: Option<String>,
    pub battery: Option<BatteryStatus>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeviceSettings {
    pub sleep_time_secs: Option<u32>,
    pub standby_status: Option<u8>,
    pub standby_time_secs: Option<u32>,
    pub mic_enabled: Option<bool>,
    pub noise_reduction: Option<(u8, u16, u16)>,
    pub motor_strength: Option<u16>,
    pub audio_button_mode: Option<u8>,
    pub hooks_mode: Option<u8>,
    pub indicator_lights: Option<IndicatorLights>,
}
