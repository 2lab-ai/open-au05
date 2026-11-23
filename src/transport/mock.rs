//! Simulated in-memory AU05 dongle + mic transport for deterministic testing.

use crate::protocol::{
    Control, FrameCodec, IndicatorLights, LedState, ShortcutKey, AUTH_PRIVATE_KEYS,
};
use crate::transport::{Transport, TransportError};
use std::collections::{BTreeMap, VecDeque};

pub struct MockTransport {
    connected: bool,
    pub mic_linked: bool,
    pub battery_percent: u16,
    pub battery_mv: u16,
    pub charging: bool,
    pub sleep_time: u32,
    pub standby_status: u8,
    pub standby_time: u32,
    pub mic_enabled: bool,
    pub noise_reduction: (u8, u16, u16),
    pub motor_strength: u16,
    pub audio_button_mode: u8,
    pub hooks_mode: u8,
    pub lights: IndicatorLights,
    pub shortcuts: BTreeMap<u8, Vec<ShortcutKey>>,
    pub fixed_functions: BTreeMap<u8, u32>,
    pub sent_plain: Vec<Vec<u8>>,
    inbox: VecDeque<Vec<u8>>,
}

impl Default for MockTransport {
    fn default() -> Self {
        let mut fixed = BTreeMap::new();
        for c in Control::ALL {
            fixed.insert(c.index(), c.factory_fixed_function());
        }
        Self {
            connected: false,
            mic_linked: true,
            battery_percent: 87,
            battery_mv: 4050,
            charging: false,
            sleep_time: 3600,
            standby_status: 0,
            standby_time: 600,
            mic_enabled: true,
            noise_reduction: (1, 1600, 1000),
            motor_strength: 0,
            audio_button_mode: 0,
            hooks_mode: 0,
            lights: IndicatorLights {
                mode: 2,
                all_on_brightness: 10,
                leds: [
                    LedState {
                        work_type: 2,
                        work_time: 10,
                        breathe_level: 2,
                        breathe_brightness: 2,
                        always_on_brightness: 2,
                    },
                    LedState {
                        work_type: 0,
                        work_time: 255,
                        breathe_level: 2,
                        breathe_brightness: 2,
                        always_on_brightness: 2,
                    },
                    LedState {
                        work_type: 0,
                        work_time: 255,
                        breathe_level: 2,
                        breathe_brightness: 2,
                        always_on_brightness: 2,
                    },
                    LedState {
                        work_type: 1,
                        work_time: 255,
                        breathe_level: 2,
                        breathe_brightness: 2,
                        always_on_brightness: 2,
                    },
                ],
            },
            shortcuts: BTreeMap::new(),
            fixed_functions: fixed,
            sent_plain: Vec::new(),
            inbox: VecDeque::new(),
        }
    }
}

impl MockTransport {
    pub fn push_plain_frame(&mut self, plain: &[u8]) {
        self.inbox
            .push_back(FrameCodec::encode_output_report(plain).to_vec());
    }

    pub fn push_key_event(&mut self, control: Control, pressed: bool) {
        self.push_plain_frame(&[
            0x0B,
            0x10,
            control.index(),
            u8::from(pressed),
            control.index(),
        ]);
    }

    fn handle_plain(&mut self, plain: &[u8]) {
        let msg_type = plain[0] & 0x1F;
        let group = plain[1] & 0x0F;
        let cmd = plain[2];
        let op = plain[3];

        match (msg_type, group, cmd, op) {
            // Dongle auth
            (0x06, 0x02, 0x05, 0x01) => {
                let idx = usize::from(plain[4] & 0x0F);
                let code = u32::from_le_bytes([plain[5], plain[6], plain[7], plain[8]]);
                let xored = code ^ AUTH_PRIVATE_KEYS[idx];
                let mut reply = vec![0x06, 0x02, 0x05, 0x11, idx as u8];
                reply.extend_from_slice(&xored.to_le_bytes());
                self.push_plain_frame(&reply);
            }
            // Heartbeat echo
            (0x06, 0x01, 0x23, 0x00) => {
                self.push_plain_frame(&[0x06, 0x01, 0x23, 0x00, 0x01]);
            }
            // Dongle version
            (0x06, 0x02, 0x03, 0x01) => {
                let mut r = vec![0u8; 20];
                r[..4].copy_from_slice(&[0x06, 0x02, 0x03, 0x11]);
                r[4..7].copy_from_slice(&[0x41, 0x00, 0x00]);
                r[10..13].copy_from_slice(&[4, 4, 0]);
                r[13..18].copy_from_slice(&[0x26, 0x07, 0x23, 0x14, 0x10]);
                self.push_plain_frame(&r);
            }
            // Dongle flash ID
            (0x06, 0x02, 0x0B, 0x01) => {
                let mut r = vec![0u8; 20];
                r[..4].copy_from_slice(&[0x06, 0x02, 0x0B, 0x11]);
                self.push_plain_frame(&r);
            }
            // Dongle serial
            (0x06, 0x02, 0x81, 0x01) => {
                let sn = b"AU05MOCK1234";
                let mut r = vec![0u8; 8 + sn.len()];
                r[..4].copy_from_slice(&[0x06, 0x02, 0x81, 0x11]);
                r[4] = sn.len() as u8;
                r[8..8 + sn.len()].copy_from_slice(sn);
                self.push_plain_frame(&r);
            }
            // Dongle device active
            (0x06, 0x03, 0x0A, 0x01) => {
                self.push_plain_frame(&[0x06, 0x03, 0x0A, 0x11, u8::from(self.mic_linked)]);
            }
            // Dongle charging
            (0x06, 0x08, 0x02, 0x01) => {
                self.push_plain_frame(&[0x06, 0x08, 0x02, 0x11, 0]);
            }
            // Dongle reboot
            (0x06, 0x01, 0x26, 0x00) => {
                self.push_plain_frame(&[0x06, 0x01, 0x26, 0x10]);
            }
            // Device battery
            (0x01, 0x01, 0x02, 0x01) if self.mic_linked => {
                let mut r = vec![0u8; 12];
                r[..4].copy_from_slice(&[0x81, 0x01, 0x02, 0x11]);
                r[4..6].copy_from_slice(&self.battery_mv.to_le_bytes());
                r[6..8].copy_from_slice(&self.battery_percent.to_le_bytes());
                r[10] = u8::from(self.charging);
                self.push_plain_frame(&r);
            }
            // Device version
            (0x01, 0x04, 0x04, 0x01) if self.mic_linked => {
                let mut r = vec![0u8; 20];
                r[..4].copy_from_slice(&[0x81, 0x04, 0x04, 0x11]);
                r[4..7].copy_from_slice(&[0x41, 0x00, 0x00]);
                r[10..13].copy_from_slice(&[4, 4, 0]);
                r[13..18].copy_from_slice(&[0x26, 0x07, 0x23, 0x14, 0x10]);
                self.push_plain_frame(&r);
            }
            // Device hardware version
            (0x01, 0x01, 0x41, 0x01) if self.mic_linked => {
                self.push_plain_frame(&[0x81, 0x01, 0x41, 0x11, 1]);
            }
            // Device flash ID
            (0x01, 0x04, 0x0B, 0x01) if self.mic_linked => {
                let mut r = vec![0u8; 20];
                r[..4].copy_from_slice(&[0x81, 0x04, 0x0B, 0x11]);
                r[4..8].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
                self.push_plain_frame(&r);
            }
            // Device serial chunks
            (0x01, 0x01, 0x0B, 0x01) if self.mic_linked => {
                let mut r0 = vec![0x81, 0x01, 0x0B, 0x11, 6, 0];
                r0.extend_from_slice(b"MIC001");
                let mut r1 = vec![0x81, 0x01, 0x0B, 0x11, 4, 1];
                r1.extend_from_slice(b"9999");
                self.push_plain_frame(&r0);
                self.push_plain_frame(&r1);
            }
            // Device UUID chunks
            (0x01, 0x01, 0x0A, 0x01) if self.mic_linked => {
                let mut r0 = vec![0x81, 0x01, 0x0A, 0x11, 8, 0];
                r0.extend_from_slice(b"UUID-001");
                self.push_plain_frame(&r0);
            }
            // Sleep time read/write
            (0x01, 0x01, 0x42, 0x01) if self.mic_linked => {
                let mut r = vec![0x81, 0x01, 0x42, 0x11];
                r.extend_from_slice(&self.sleep_time.to_le_bytes());
                self.push_plain_frame(&r);
            }
            (0x01, 0x01, 0x42, 0x02) if self.mic_linked => {
                self.sleep_time = u32::from_le_bytes([plain[4], plain[5], plain[6], plain[7]]);
                self.push_plain_frame(&[0x81, 0x01, 0x42, 0x12]);
            }
            // Standby status
            (0x01, 0x01, 0x0D, 0x01) if self.mic_linked => {
                self.push_plain_frame(&[0x81, 0x01, 0x0D, 0x11, self.standby_status]);
            }
            (0x01, 0x01, 0x0D, 0x00) if self.mic_linked => {
                self.standby_status = plain[4];
                self.push_plain_frame(&[0x81, 0x01, 0x0D, 0x10]);
            }
            // Standby time
            (0x01, 0x01, 0x2C, 0x01) if self.mic_linked => {
                let mut r = vec![0x81, 0x01, 0x2C, 0x11];
                r.extend_from_slice(&self.standby_time.to_le_bytes());
                self.push_plain_frame(&r);
            }
            (0x01, 0x01, 0x2C, 0x02) if self.mic_linked => {
                self.standby_time = u32::from_le_bytes([plain[4], plain[5], plain[6], plain[7]]);
                self.push_plain_frame(&[0x81, 0x01, 0x2C, 0x12]);
            }
            // Mic enable
            (0x01, 0x01, 0x2A, 0x01) if self.mic_linked => {
                self.push_plain_frame(&[0x81, 0x01, 0x2A, 0x11, u8::from(self.mic_enabled)]);
            }
            (0x01, 0x01, 0x2A, 0x00) if self.mic_linked => {
                self.mic_enabled = plain[4] != 0;
                self.push_plain_frame(&[0x81, 0x01, 0x2A, 0x10]);
            }
            // Noise reduction
            (0x01, 0x01, 0x90, 0x01) if self.mic_linked => {
                let (lvl, low, high) = self.noise_reduction;
                let mut r = vec![0x81, 0x01, 0x90, 0x11, lvl];
                r.extend_from_slice(&low.to_le_bytes());
                r.extend_from_slice(&high.to_le_bytes());
                self.push_plain_frame(&r);
            }
            (0x01, 0x01, 0x90, 0x04) if self.mic_linked => {
                self.noise_reduction = (
                    plain[4],
                    u16::from_le_bytes([plain[5], plain[6]]),
                    u16::from_le_bytes([plain[7], plain[8]]),
                );
                self.push_plain_frame(&[0x81, 0x01, 0x90, 0x14]);
            }
            // Motor strength
            (0x01, 0x06, 0x40, 0x01) if self.mic_linked => {
                let mut r = vec![0x81, 0x06, 0x40, 0x11];
                r.extend_from_slice(&self.motor_strength.to_le_bytes());
                self.push_plain_frame(&r);
            }
            (0x01, 0x06, 0x40, 0x04) if self.mic_linked => {
                self.motor_strength = u16::from_le_bytes([plain[4], plain[5]]);
                self.push_plain_frame(&[0x81, 0x06, 0x40, 0x14]);
            }
            // Audio button mode
            (0x01, 0x06, 0x51, 0x01) if self.mic_linked => {
                self.push_plain_frame(&[0x81, 0x06, 0x51, 0x11, self.audio_button_mode]);
            }
            (0x01, 0x06, 0x51, 0x04) if self.mic_linked => {
                self.audio_button_mode = plain[4];
                self.push_plain_frame(&[0x81, 0x06, 0x51, 0x14]);
            }
            // Hooks mode
            (0x01, 0x0B, 0x89, 0x01) if self.mic_linked => {
                self.push_plain_frame(&[0x81, 0x0B, 0x89, 0x11, self.hooks_mode]);
            }
            (0x01, 0x0B, 0x89, 0x04) if self.mic_linked => {
                self.hooks_mode = plain[4];
                self.push_plain_frame(&[0x81, 0x0B, 0x89, 0x10]);
            }
            // Indicator LEDs
            (0x01, 0x0B, 0x88, 0x01) if self.mic_linked => {
                let mut r = vec![0u8; 28];
                r[..4].copy_from_slice(&[0x81, 0x0B, 0x88, 0x11]);
                r[6] = self.lights.mode;
                r[7] = self.lights.all_on_brightness;
                for (i, led) in self.lights.leds.iter().enumerate() {
                    let o = 8 + 5 * i;
                    r[o] = led.work_type;
                    r[o + 1] = led.work_time;
                    r[o + 2] = led.breathe_level;
                    r[o + 3] = led.breathe_brightness;
                    r[o + 4] = led.always_on_brightness;
                }
                self.push_plain_frame(&r);
            }
            (0x01, 0x0B, 0x88, 0x04) if self.mic_linked => {
                let mask = plain[4];
                let led = usize::from(plain[5]);
                if mask == 0x01 {
                    self.lights.mode = plain[6];
                } else if mask == 0x02 {
                    self.lights.all_on_brightness = plain[7];
                } else if led < 4 {
                    let o = 8 + 5 * led;
                    if mask == 0x04 {
                        self.lights.leds[led].work_type = plain[o];
                    } else if mask == 0x08 {
                        self.lights.leds[led].work_time = plain[o + 1];
                    } else if mask == 0x10 {
                        self.lights.leds[led].breathe_level = plain[o + 2];
                    } else if mask == 0x20 {
                        self.lights.leds[led].breathe_brightness = plain[o + 3];
                    } else if mask == 0x40 {
                        self.lights.leds[led].always_on_brightness = plain[o + 4];
                    }
                }
                self.push_plain_frame(&[0x81, 0x0B, 0x88, 0x14]);
            }
            // Button shortcut read/write
            (0x01, 0x06, 0x50, 0x01) if self.mic_linked => {
                let idx = plain[4];
                let keys = self.shortcuts.get(&idx).cloned().unwrap_or_default();
                let mut r = vec![0x81, 0x06, 0x50, 0x11, idx, 0x01, keys.len() as u8];
                for k in keys {
                    r.push((k.page & 0x7F) | if k.sign { 0x80 } else { 0 });
                    r.push(k.value);
                }
                self.push_plain_frame(&r);
            }
            (0x01, 0x06, 0x50, 0x04) if self.mic_linked => {
                let idx = plain[4];
                let count = usize::from(plain[6]).min(4);
                let mut keys = Vec::with_capacity(count);
                for i in 0..count {
                    let p = plain[7 + 2 * i];
                    keys.push(ShortcutKey::new(p & 0x7F, plain[8 + 2 * i], p & 0x80 != 0));
                }
                self.shortcuts.insert(idx, keys);
                self.push_plain_frame(&[0x81, 0x06, 0x50, 0x14, idx]);
            }
            // Button fixed function read/write
            (0x01, 0x06, 0x10, 0x01) if self.mic_linked => {
                let idx = plain[5];
                let func = self.fixed_functions.get(&idx).copied().unwrap_or(0);
                let mut r = vec![0x81, 0x06, 0x10, 0x11, 0, idx];
                r.extend_from_slice(&func.to_le_bytes());
                self.push_plain_frame(&r);
            }
            (0x01, 0x06, 0x10, 0x04) if self.mic_linked => {
                let idx = plain[5];
                let func = u32::from_le_bytes([plain[6], plain[7], plain[8], plain[9]]);
                self.fixed_functions.insert(idx, func);
                self.push_plain_frame(&[0x81, 0x06, 0x10, 0x14, 0, idx]);
            }
            // Device reboot
            (0x01, 0x01, 0x0C, 0x00) if self.mic_linked => {
                self.push_plain_frame(&[0x81, 0x01, 0x0C, 0x10]);
            }
            _ => {}
        }
    }
}

impl Transport for MockTransport {
    fn connect(&mut self) -> Result<(), TransportError> {
        self.connected = true;
        Ok(())
    }

    fn read_report(&mut self) -> Result<Option<Vec<u8>>, TransportError> {
        if !self.connected {
            return Err(TransportError::NotFound("mock disconnected".into()));
        }
        Ok(self.inbox.pop_front())
    }

    fn write_report(&mut self, report: &[u8]) -> Result<(), TransportError> {
        if !self.connected {
            return Err(TransportError::NotFound("mock disconnected".into()));
        }
        let frame = FrameCodec::decode_input_report(report)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        self.sent_plain.push(frame.bytes.clone());
        self.handle_plain(&frame.bytes);
        Ok(())
    }

    fn close(&mut self) {
        self.connected = false;
    }

    fn device_path(&self) -> Option<&str> {
        self.connected.then_some("/dev/hidraw-mock")
    }
}
