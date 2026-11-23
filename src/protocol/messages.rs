//! Parser for all AU05 replies, notices, and firmware upgrade events.

use crate::protocol::hotkey::ShortcutKey;
use crate::protocol::tea::Frame;
use crate::protocol::types::{
    BatteryStatus, IndicatorLights, KeyEvent, LedState, MessageType, VersionInfo, AUTH_PRIVATE_KEYS,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpgradeEvent {
    Connected,
    ReceivePackageNum { mask: u32, checksum: u32 },
    ProgramComplete { result: u8, mask: u8, index: u16 },
    CheckAllSum(u32),
    Result(u8),
    Other(Frame),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VibeMessage {
    // Dongle replies
    AuthReply {
        key_index: usize,
        code: u32,
    },
    DongleVersion(VersionInfo),
    DongleFlashId(Vec<u8>),
    DongleSerial(String),
    DeviceActive(bool),
    DongleCharging(u8),
    DongleReboot,
    Heartbeat,

    // Device (mic) replies
    DeviceVersion(VersionInfo),
    HardwareVersion(u8),
    DeviceFlashId(Vec<u8>),
    DeviceSerialChunk {
        offset: usize,
        bytes: Vec<u8>,
    },
    UuidChunk {
        offset: usize,
        bytes: Vec<u8>,
    },
    MacAddress(Vec<u8>),
    Battery(BatteryStatus),
    StandbyStatus(u8),
    StandbyTime(u32),
    SleepTime(u32),
    MicEnable(bool),
    NoiseReduction {
        level: u8,
        low: u16,
        high: u16,
    },
    MotorStrength(u16),
    AudioButtonMode(u8),
    HooksMode(u8),
    IndicatorLights(IndicatorLights),
    ButtonFixedFunction {
        index: u8,
        macro_config: u8,
        function: u32,
    },
    ButtonShortcut {
        index: u8,
        keys: Vec<ShortcutKey>,
    },
    SupportFlag {
        command: u8,
        value: u8,
    },
    DeviceReboot,

    // Notices (0x0B)
    KeyEvent(KeyEvent),
    LinkActive(bool),
    BatteryNotice(BatteryStatus),
    ChargingNotice(bool),
    StandbyNotice(u8),
    PowerOn,
    NoiseReductionNotice {
        level: u8,
        low: i16,
        high: i16,
    },

    // Firmware upgrade
    Upgrade {
        target: u8,
        event: UpgradeEvent,
    },

    // Generic
    Reply(Frame),
    Unknown(Frame),
}

impl VibeMessage {
    pub fn parse(f: &Frame) -> Self {
        match f.raw_type() & 0x1F {
            x if x == MessageType::Dongle as u8 => Self::parse_dongle(f),
            x if x == MessageType::Device as u8 => Self::parse_device(f),
            x if x == MessageType::Notice as u8 => Self::parse_notice(f),
            x if x == MessageType::DongleUpgrade as u8
                || x == MessageType::DeviceUpgrade as u8
                || x == MessageType::UploadImage as u8 =>
            {
                Self::Upgrade {
                    target: x,
                    event: Self::parse_upgrade(f),
                }
            }
            _ => Self::Unknown(f.clone()),
        }
    }

    fn parse_dongle(f: &Frame) -> Self {
        if f.group() == 0x01 && f.command() == 0x23 {
            return Self::Heartbeat;
        }
        if !f.is_reply() {
            return Self::Unknown(f.clone());
        }
        match (f.group(), f.command()) {
            (0x01, 0x26) => Self::DongleReboot,
            (0x02, 0x03) => Self::DongleVersion(parse_version(f)),
            (0x02, 0x05) => {
                let idx = usize::from(f.byte(4));
                if idx >= AUTH_PRIVATE_KEYS.len() {
                    return Self::Unknown(f.clone());
                }
                Self::AuthReply {
                    key_index: idx,
                    code: f.u32_le(5) ^ AUTH_PRIVATE_KEYS[idx],
                }
            }
            (0x02, 0x0B) => Self::DongleFlashId(f.slice(4, 16)),
            (0x02, 0x81) => {
                let len = usize::from(f.byte(4)).min(30);
                Self::DongleSerial(ascii_str(&f.slice(8, len)))
            }
            (0x03, 0x0A) => Self::DeviceActive(f.byte(4) != 0),
            (0x08, 0x02) => Self::DongleCharging(f.byte(4)),
            _ => Self::Reply(f.clone()),
        }
    }

    fn parse_device(f: &Frame) -> Self {
        if !f.is_reply() {
            return Self::Unknown(f.clone());
        }
        match (f.group(), f.command()) {
            (0x01, 0x02) => {
                let flags = f.byte(11);
                Self::Battery(BatteryStatus {
                    millivolts: f.u16_le(4),
                    percent: f.u16_le(6),
                    charging: f.byte(10) != 0,
                    charge_full: flags & 0x08 != 0,
                    low_battery_level: flags & 0x03,
                    power_off: flags & 0x04 != 0,
                })
            }
            (0x01, 0x0A) => Self::UuidChunk {
                offset: usize::from(f.byte(5)),
                bytes: f.slice(6, usize::from(f.byte(4)).min(58)),
            },
            (0x01, 0x0B) => Self::DeviceSerialChunk {
                offset: usize::from(f.byte(5)),
                bytes: f.slice(6, usize::from(f.byte(4)).min(58)),
            },
            (0x01, 0x0C) => Self::DeviceReboot,
            (0x01, 0x0D) => {
                if f.operation() == 0x11 {
                    Self::StandbyStatus(f.byte(4))
                } else {
                    Self::Reply(f.clone())
                }
            }
            (0x01, 0x2A) => {
                if f.operation() == 0x11 {
                    Self::MicEnable(f.byte(4) != 0)
                } else {
                    Self::Reply(f.clone())
                }
            }
            (0x01, 0x2C) => {
                if f.operation() == 0x11 {
                    Self::StandbyTime(f.u32_le(4))
                } else {
                    Self::Reply(f.clone())
                }
            }
            (0x01, 0x41) => Self::HardwareVersion(f.byte(4)),
            (0x01, 0x42) => {
                if f.operation() == 0x11 {
                    Self::SleepTime(f.u32_le(4))
                } else {
                    Self::Reply(f.clone())
                }
            }
            (0x01, 0x90) => {
                if f.operation() == 0x11 {
                    Self::NoiseReduction {
                        level: f.byte(4),
                        low: f.u16_le(5),
                        high: f.u16_le(7),
                    }
                } else {
                    Self::Reply(f.clone())
                }
            }
            (0x01, 0xFA) => Self::MacAddress(f.slice(4, 6)),
            (0x04, 0x04) => Self::DeviceVersion(parse_version(f)),
            (0x04, 0x0B) => Self::DeviceFlashId(f.slice(4, 16)),
            (0x06, 0x10) => Self::ButtonFixedFunction {
                index: f.byte(5),
                macro_config: f.byte(4),
                function: f.u32_le(6),
            },
            (0x06, 0x37) | (0x06, 0x38) | (0x06, 0x39) => Self::SupportFlag {
                command: f.command(),
                value: f.byte(4),
            },
            (0x06, 0x40) => {
                if f.operation() == 0x11 {
                    Self::MotorStrength(f.u16_le(4))
                } else {
                    Self::Reply(f.clone())
                }
            }
            (0x06, 0x50) => {
                let count = usize::from(f.byte(6)).min(24);
                let mut keys = Vec::with_capacity(count);
                for i in 0..count {
                    let page_byte = f.byte(7 + 2 * i);
                    keys.push(ShortcutKey::new(
                        page_byte & 0x7F,
                        f.byte(8 + 2 * i),
                        page_byte & 0x80 != 0,
                    ));
                }
                Self::ButtonShortcut {
                    index: f.byte(4),
                    keys,
                }
            }
            (0x06, 0x51) => {
                if f.operation() == 0x11 {
                    Self::AudioButtonMode(f.byte(4))
                } else {
                    Self::Reply(f.clone())
                }
            }
            (0x0B, 0x88) => {
                if f.operation() != 0x11 {
                    return Self::Reply(f.clone());
                }
                let mut leds = Vec::with_capacity(4);
                for i in 0..4 {
                    let o = 8 + 5 * i;
                    leds.push(LedState {
                        work_type: f.byte(o),
                        work_time: f.byte(o + 1),
                        breathe_level: f.byte(o + 2),
                        breathe_brightness: f.byte(o + 3),
                        always_on_brightness: f.byte(o + 4),
                    });
                }
                let leds: [LedState; 4] = leds.try_into().unwrap();
                Self::IndicatorLights(IndicatorLights {
                    mode: f.byte(6),
                    all_on_brightness: f.byte(7),
                    leds,
                })
            }
            (0x0B, 0x89) => {
                if f.operation() == 0x11 {
                    Self::HooksMode(f.byte(4))
                } else {
                    Self::Reply(f.clone())
                }
            }
            _ => Self::Reply(f.clone()),
        }
    }

    fn parse_notice(f: &Frame) -> Self {
        match f.byte(1) {
            0x0B => Self::LinkActive(f.byte(2) & 1 != 0),
            0x0D => Self::StandbyNotice(f.byte(2)),
            0x10 => Self::KeyEvent(KeyEvent {
                index: f.byte(2),
                status: f.byte(3),
                physical_index: f.byte(4),
            }),
            0x6F => Self::ChargingNotice(f.byte(4) & 0x08 != 0),
            0x7B => {
                let flags = f.byte(4);
                Self::BatteryNotice(BatteryStatus {
                    millivolts: f.u16_le(2),
                    percent: u16::from(f.byte(5)),
                    charging: flags & 0x08 != 0,
                    charge_full: flags & 0x10 != 0,
                    low_battery_level: flags & 0x03,
                    power_off: flags & 0x04 != 0,
                })
            }
            0x83 => Self::NoiseReductionNotice {
                level: f.byte(2),
                low: f.u16_le(3) as i16,
                high: f.u16_le(5) as i16,
            },
            0xF0 => Self::PowerOn,
            _ => Self::Unknown(f.clone()),
        }
    }

    fn parse_upgrade(f: &Frame) -> UpgradeEvent {
        match f.byte(1) {
            0x01 => UpgradeEvent::Connected,
            0x03 => UpgradeEvent::ReceivePackageNum {
                mask: f.u32_le(2),
                checksum: f.u32_le(6),
            },
            0x05 => UpgradeEvent::ProgramComplete {
                result: f.byte(2),
                mask: f.byte(3),
                index: f.u16_le(4),
            },
            0x06 => UpgradeEvent::CheckAllSum(f.u32_le(2)),
            0x08 => UpgradeEvent::Result(f.byte(2)),
            _ => UpgradeEvent::Other(f.clone()),
        }
    }
}

fn parse_version(f: &Frame) -> VersionInfo {
    let p = f.slice(4, 15);
    let b = |i: usize| p.get(i).copied().unwrap_or(0);
    let custom_code = format!("{:02X}{:02X}{:02X}", b(0), b(1), b(2));
    let version = if b(14) != 0 {
        format!("{}.{}.{}.{}", b(6), b(7), b(8), b(14))
    } else {
        format!("{}.{}.{}", b(6), b(7), b(8))
    };
    let build_stamp = format!(
        "20{:02x}-{:02x}-{:02x} {:02x}:{:02x}",
        b(9),
        b(10),
        b(11),
        b(12),
        b(13)
    );
    VersionInfo {
        custom_code,
        version,
        build_stamp,
        raw: p,
    }
}

fn ascii_str(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

pub fn hex_string(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}
