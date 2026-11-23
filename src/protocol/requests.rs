//! Request builders and reply matchers for the AU05 vendor HID protocol.

use crate::protocol::hotkey::ShortcutKey;
use crate::protocol::tea::Frame;
use crate::protocol::types::{LedField, MessageType, Operation, REPORT_LENGTH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyMatch {
    pub msg_type: u8,
    pub group: u8,
    pub command: u8,
    pub byte_check: Option<(usize, u8)>,
    pub operation: Option<u8>,
}

impl ReplyMatch {
    pub fn matches(&self, f: &Frame) -> bool {
        if (f.raw_type() & 0x1F) != self.msg_type
            || f.group() != self.group
            || f.command() != self.command
            || !f.is_reply()
        {
            return false;
        }
        if let Some(expected_op) = self.operation {
            if f.operation() != expected_op {
                let is_write = expected_op != (Operation::READ | Operation::REPLY_FLAG);
                if !(is_write && f.operation() == Operation::REPLY_FLAG) {
                    return false;
                }
            }
        }
        if let Some((idx, val)) = self.byte_check {
            if f.byte(idx) != val {
                return false;
            }
        }
        true
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub name: &'static str,
    pub bytes: [u8; REPORT_LENGTH],
    pub reply: Option<ReplyMatch>,
}

impl Request {
    pub fn new(name: &'static str, data: &[u8], reply: Option<ReplyMatch>) -> Self {
        let mut bytes = [0u8; REPORT_LENGTH];
        let n = data.len().min(REPORT_LENGTH);
        bytes[..n].copy_from_slice(&data[..n]);
        Self { name, bytes, reply }
    }

    fn simple(
        name: &'static str,
        msg_type: MessageType,
        group: u8,
        command: u8,
        op: u8,
        payload: &[u8],
        byte_check: Option<(usize, u8)>,
    ) -> Self {
        let mut data = Vec::with_capacity(4 + payload.len());
        data.push(msg_type as u8);
        data.push(group);
        data.push(command);
        data.push(op);
        data.extend_from_slice(payload);
        Self::new(
            name,
            &data,
            Some(ReplyMatch {
                msg_type: msg_type as u8,
                group,
                command,
                byte_check,
                operation: Some(op | Operation::REPLY_FLAG),
            }),
        )
    }
}

/// Commands handled by the USB receiver dongle (`type = 0x06`).
pub struct DongleRequest;

impl DongleRequest {
    pub fn auth(code: u32) -> Request {
        let mut payload = [0u8; 5];
        payload[0] = (code & 0x0F) as u8;
        payload[1..5].copy_from_slice(&code.to_le_bytes());
        Request::simple(
            "dongle.auth",
            MessageType::Dongle,
            0x02,
            0x05,
            Operation::READ,
            &payload,
            None,
        )
    }

    pub fn heartbeat() -> Request {
        Request::new("dongle.heartbeat", &[0x06, 0x01, 0x23, 0x00, 0x01], None)
    }

    pub fn version() -> Request {
        Request::simple(
            "dongle.version",
            MessageType::Dongle,
            0x02,
            0x03,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn flash_id() -> Request {
        Request::simple(
            "dongle.flash_id",
            MessageType::Dongle,
            0x02,
            0x0B,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn serial_number() -> Request {
        Request::simple(
            "dongle.serial",
            MessageType::Dongle,
            0x02,
            0x81,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn device_active() -> Request {
        Request::simple(
            "dongle.device_active",
            MessageType::Dongle,
            0x03,
            0x0A,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn charging_status() -> Request {
        Request::simple(
            "dongle.charging",
            MessageType::Dongle,
            0x08,
            0x02,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn reboot(delay_ms: u32) -> Request {
        Request::simple(
            "dongle.reboot",
            MessageType::Dongle,
            0x01,
            0x26,
            Operation::WRITE0,
            &delay_ms.to_le_bytes(),
            None,
        )
    }
}

/// Commands relayed to the AU05 microphone (`type = 0x01`).
pub struct DeviceRequest;

impl DeviceRequest {
    /// Take over key events so the dongle forwards `0x0B 0x10` notices to the host.
    pub fn takeover() -> Request {
        Request::new("device.takeover", &[0x01, 0x01, 0x10, 0x00, 0x03], None)
    }

    /// Return control to the dongle so onboard HID bindings resume immediately.
    pub fn hand_back() -> Request {
        Request::new("device.hand_back", &[0x01, 0x01, 0x10, 0x00, 0x00], None)
    }

    pub fn battery() -> Request {
        Request::simple(
            "device.battery",
            MessageType::Device,
            0x01,
            0x02,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn version() -> Request {
        Request::simple(
            "device.version",
            MessageType::Device,
            0x04,
            0x04,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn hardware_version() -> Request {
        Request::simple(
            "device.hw_version",
            MessageType::Device,
            0x01,
            0x41,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn flash_id() -> Request {
        Request::simple(
            "device.flash_id",
            MessageType::Device,
            0x04,
            0x0B,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn serial_number() -> Request {
        Request::simple(
            "device.serial",
            MessageType::Device,
            0x01,
            0x0B,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn uuid() -> Request {
        Request::simple(
            "device.uuid",
            MessageType::Device,
            0x01,
            0x0A,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn mac_address() -> Request {
        Request::simple(
            "device.mac",
            MessageType::Device,
            0x01,
            0xFA,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn standby_status() -> Request {
        Request::simple(
            "device.standby",
            MessageType::Device,
            0x01,
            0x0D,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn set_standby_status(on: bool) -> Request {
        Request::simple(
            "device.set_standby",
            MessageType::Device,
            0x01,
            0x0D,
            Operation::WRITE0,
            &[u8::from(on)],
            None,
        )
    }

    pub fn standby_time() -> Request {
        Request::simple(
            "device.standby_time",
            MessageType::Device,
            0x01,
            0x2C,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn set_standby_time(seconds: u32) -> Request {
        Request::simple(
            "device.set_standby_time",
            MessageType::Device,
            0x01,
            0x2C,
            Operation::WRITE2,
            &seconds.to_le_bytes(),
            None,
        )
    }

    pub fn sleep_time() -> Request {
        Request::simple(
            "device.sleep_time",
            MessageType::Device,
            0x01,
            0x42,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn set_sleep_time(seconds: u32) -> Request {
        Request::simple(
            "device.set_sleep_time",
            MessageType::Device,
            0x01,
            0x42,
            Operation::WRITE2,
            &seconds.to_le_bytes(),
            None,
        )
    }

    pub fn mic_enable() -> Request {
        Request::simple(
            "device.mic_enable",
            MessageType::Device,
            0x01,
            0x2A,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn set_mic_enable(on: bool) -> Request {
        Request::simple(
            "device.set_mic_enable",
            MessageType::Device,
            0x01,
            0x2A,
            Operation::WRITE0,
            &[u8::from(on)],
            None,
        )
    }

    pub fn noise_reduction() -> Request {
        Request::simple(
            "device.noise_reduction",
            MessageType::Device,
            0x01,
            0x90,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn set_noise_reduction(level: u8, low: u16, high: u16) -> Request {
        let mut payload = [0u8; 5];
        payload[0] = level;
        payload[1..3].copy_from_slice(&low.to_le_bytes());
        payload[3..5].copy_from_slice(&high.to_le_bytes());
        Request::simple(
            "device.set_noise_reduction",
            MessageType::Device,
            0x01,
            0x90,
            Operation::WRITE4,
            &payload,
            None,
        )
    }

    pub fn motor_strength() -> Request {
        Request::simple(
            "device.motor_strength",
            MessageType::Device,
            0x06,
            0x40,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn set_motor_strength(value: u16) -> Request {
        Request::simple(
            "device.set_motor_strength",
            MessageType::Device,
            0x06,
            0x40,
            Operation::WRITE4,
            &value.to_le_bytes(),
            None,
        )
    }

    pub fn audio_button_mode() -> Request {
        Request::simple(
            "device.audio_button_mode",
            MessageType::Device,
            0x06,
            0x51,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn set_audio_button_mode(mode: u8) -> Request {
        Request::simple(
            "device.set_audio_button_mode",
            MessageType::Device,
            0x06,
            0x51,
            Operation::WRITE4,
            &[mode],
            None,
        )
    }

    pub fn hooks_mode() -> Request {
        Request::simple(
            "device.hooks_mode",
            MessageType::Device,
            0x0B,
            0x89,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn set_hooks_mode(on: bool) -> Request {
        Request::simple(
            "device.set_hooks_mode",
            MessageType::Device,
            0x0B,
            0x89,
            Operation::WRITE4,
            &[u8::from(on)],
            None,
        )
    }

    pub fn indicator_lights() -> Request {
        Request::simple(
            "device.indicator_lights",
            MessageType::Device,
            0x0B,
            0x88,
            Operation::READ,
            &[],
            None,
        )
    }

    pub fn set_light_mode(mode: u8) -> Request {
        Request::simple(
            "device.set_light_mode",
            MessageType::Device,
            0x0B,
            0x88,
            Operation::WRITE4,
            &[0x01, 0x00, mode],
            None,
        )
    }

    pub fn set_all_on_brightness(level: u8) -> Request {
        Request::simple(
            "device.set_all_on_brightness",
            MessageType::Device,
            0x0B,
            0x88,
            Operation::WRITE4,
            &[0x02, 0x00, 0x00, level],
            None,
        )
    }

    pub fn set_led_field(led: u8, field: LedField, value: u8) -> Request {
        let mut data = [0u8; REPORT_LENGTH];
        data[0] = MessageType::Device as u8;
        data[1] = 0x0B;
        data[2] = 0x88;
        data[3] = Operation::WRITE4;
        data[4] = field.mask();
        data[5] = led;
        let offset = 8 + 5 * usize::from(led) + (field as usize);
        if offset < REPORT_LENGTH {
            data[offset] = value;
        }
        Request::new(
            "device.set_led_field",
            &data,
            Some(ReplyMatch {
                msg_type: MessageType::Device as u8,
                group: 0x0B,
                command: 0x88,
                byte_check: None,
                operation: Some(Operation::WRITE4 | Operation::REPLY_FLAG),
            }),
        )
    }

    pub fn button_fixed_function(index: u8) -> Request {
        Request::simple(
            "device.button_fixed_function",
            MessageType::Device,
            0x06,
            0x10,
            Operation::READ,
            &[0x00, index],
            Some((5, index)),
        )
    }

    pub fn set_button_fixed_function(index: u8, function: u32) -> Request {
        let mut payload = [0u8; 6];
        payload[0] = 0x00;
        payload[1] = index;
        payload[2..6].copy_from_slice(&function.to_le_bytes());
        Request::simple(
            "device.set_button_fixed_function",
            MessageType::Device,
            0x06,
            0x10,
            Operation::WRITE4,
            &payload,
            Some((5, index)),
        )
    }

    pub fn button_shortcut(index: u8) -> Request {
        Request::simple(
            "device.button_shortcut",
            MessageType::Device,
            0x06,
            0x50,
            Operation::READ,
            &[index],
            Some((4, index)),
        )
    }

    pub fn set_button_shortcut(index: u8, keys: &[ShortcutKey]) -> Request {
        let mut payload = Vec::with_capacity(3 + keys.len() * 2);
        payload.push(index);
        payload.push(0x01);
        payload.push(keys.len() as u8);
        for k in keys {
            payload.push((k.page & 0x7F) | if k.sign { 0x80 } else { 0 });
            payload.push(k.value);
        }
        Request::simple(
            "device.set_button_shortcut",
            MessageType::Device,
            0x06,
            0x50,
            Operation::WRITE4,
            &payload,
            Some((4, index)),
        )
    }

    pub fn reboot(delay_ms: u32) -> Request {
        Request::simple(
            "device.reboot",
            MessageType::Device,
            0x01,
            0x0C,
            Operation::WRITE0,
            &delay_ms.to_le_bytes(),
            None,
        )
    }
}

/// Firmware OTA upgrade frame builders (`target` = `0x1E` dongle, `0x1F` mic).
pub struct UpgradeRequest;

impl UpgradeRequest {
    pub fn connect(target: u8, file_length: u32, custom_code: &[u8]) -> [u8; REPORT_LENGTH] {
        let mut b = [0u8; REPORT_LENGTH];
        b[0] = target;
        b[1] = 0x01;
        b[8..12].copy_from_slice(&file_length.to_le_bytes());
        for (i, &c) in custom_code.iter().take(3).enumerate() {
            b[12 + i] = c;
        }
        b
    }

    pub fn data(target: u8, package_index: u16, chunk: &[u8]) -> [u8; REPORT_LENGTH] {
        let mut b = [0u8; REPORT_LENGTH];
        b[0] = target;
        b[1] = 0x02;
        b[2..4].copy_from_slice(&package_index.to_le_bytes());
        let len = chunk.len().min(59);
        b[4] = len as u8;
        b[5..5 + len].copy_from_slice(&chunk[..len]);
        b
    }

    pub fn receive_package_num(target: u8, checksum: u32) -> [u8; REPORT_LENGTH] {
        let mut b = [0u8; REPORT_LENGTH];
        b[0] = target;
        b[1] = 0x03;
        b[6..10].copy_from_slice(&checksum.to_le_bytes());
        b
    }

    pub fn enable_program(
        target: u8,
        frame_index: u16,
        package_count: u8,
        enable: bool,
    ) -> [u8; REPORT_LENGTH] {
        let mut b = [0u8; REPORT_LENGTH];
        b[0] = target;
        b[1] = 0x04;
        b[2..4].copy_from_slice(&frame_index.to_le_bytes());
        b[4] = package_count;
        b[5] = u8::from(enable);
        b
    }

    pub fn check_all_sum(target: u8) -> [u8; REPORT_LENGTH] {
        let mut b = [0u8; REPORT_LENGTH];
        b[0] = target;
        b[1] = 0x06;
        b
    }

    pub fn all_page_complete_result(target: u8, matched: bool) -> [u8; REPORT_LENGTH] {
        let mut b = [0u8; REPORT_LENGTH];
        b[0] = target;
        b[1] = 0x07;
        b[2] = u8::from(matched);
        b
    }
}
