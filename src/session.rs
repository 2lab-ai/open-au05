//! High-level AU05 HID session: authentication handshake, 1s keepalive heartbeat,
//! retried request/reply exchange, chunk reassembly, and typed device commands.

use crate::protocol::{
    hex_string, BatteryStatus, Control, DeviceInfo, DeviceRequest, DeviceSettings, DongleRequest,
    FixedFunction, Frame, FrameCodec, Hotkey, IndicatorLights, LedField, LightMode, Request,
    ShortcutKey, VibeMessage,
};
use crate::transport::{Transport, TransportError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub enum SessionError {
    Transport(TransportError),
    AuthTimeout,
    Timeout(&'static str),
    MicNotLinked,
    Protocol(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "{e}"),
            Self::AuthTimeout => {
                f.write_str("dongle did not complete the authentication handshake")
            }
            Self::Timeout(name) => write!(
                f,
                "no reply to {name} (is the AU05 Vibe Key powered on and linked?)"
            ),
            Self::MicNotLinked => {
                f.write_str("AU05 microphone is not linked to the USB receiver (power it on)")
            }
            Self::Protocol(msg) => write!(f, "protocol error: {msg}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<TransportError> for SessionError {
    fn from(e: TransportError) -> Self {
        Self::Transport(e)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ButtonBindingInfo {
    pub control: Control,
    pub shortcut: Vec<ShortcutKey>,
    pub shortcut_text: String,
    pub fixed_function: u32,
    pub fixed_function_name: String,
}

pub struct Session {
    transport: Box<dyn Transport>,
    authenticated: bool,
    heartbeat_enabled: bool,
    last_heartbeat: Instant,
    verbose: bool,
    retry_interval: Duration,
    default_timeout: Duration,
    event_queue: VecDeque<VibeMessage>,
}

impl Session {
    pub fn new(transport: Box<dyn Transport>, verbose: bool) -> Self {
        Self {
            transport,
            authenticated: false,
            heartbeat_enabled: true,
            last_heartbeat: Instant::now() - Duration::from_secs(10),
            verbose,
            retry_interval: Duration::from_millis(100),
            default_timeout: Duration::from_secs(3),
            event_queue: VecDeque::new(),
        }
    }

    pub fn set_heartbeat_enabled(&mut self, enabled: bool) {
        self.heartbeat_enabled = enabled;
    }

    pub fn is_authenticated(&self) -> bool {
        self.authenticated
    }

    pub fn device_path(&self) -> Option<&str> {
        self.transport.device_path()
    }

    /// Runs the challenge-response authentication handshake (`06 02 05 01`).
    pub fn authenticate(&mut self, timeout: Duration) -> Result<(), SessionError> {
        let deadline = Instant::now() + timeout;
        let mut next_send = Instant::now();
        let mut code = pseudo_random_u32();

        while Instant::now() < deadline {
            if Instant::now() >= next_send {
                code = pseudo_random_u32();
                let req = DongleRequest::auth(code);
                self.send_raw(&req.bytes)?;
                next_send = Instant::now() + Duration::from_millis(500);
            }
            for msg in self.read_incoming()? {
                if let VibeMessage::AuthReply { code: echoed, .. } = msg {
                    if echoed == code {
                        self.authenticated = true;
                        self.last_heartbeat = Instant::now() - Duration::from_secs(10);
                        let _ = self.maybe_send_heartbeat();
                        return Ok(());
                    }
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        Err(SessionError::AuthTimeout)
    }

    pub fn maybe_send_heartbeat(&mut self) -> Result<(), SessionError> {
        if self.authenticated
            && self.heartbeat_enabled
            && self.last_heartbeat.elapsed() >= Duration::from_secs(1)
        {
            self.last_heartbeat = Instant::now();
            self.send_raw(&DongleRequest::heartbeat().bytes)?;
        }
        Ok(())
    }

    pub fn takeover(&mut self) -> Result<(), SessionError> {
        self.send_raw(&DeviceRequest::takeover().bytes)
    }

    pub fn hand_back(&mut self) -> Result<(), SessionError> {
        self.send_raw(&DeviceRequest::hand_back().bytes)
    }

    pub fn send_raw(&mut self, plain: &[u8; 64]) -> Result<(), SessionError> {
        if self.verbose {
            eprintln!(">> {}", Frame::new(plain.to_vec()).short_hex());
        }
        let report = FrameCodec::encode_output_report(plain);
        self.transport.write_report(&report)?;
        Ok(())
    }

    pub fn poll_raw_messages(&mut self) -> Result<Vec<VibeMessage>, SessionError> {
        self.read_incoming()
    }

    /// Polls pending async notices/events (key events, battery notices, link state, etc.).
    pub fn poll_events(&mut self) -> Result<Vec<VibeMessage>, SessionError> {
        self.maybe_send_heartbeat()?;
        let fresh = self.read_incoming()?;
        let mut out: Vec<VibeMessage> = self.event_queue.drain(..).collect();
        out.extend(fresh);
        Ok(out)
    }

    fn read_incoming(&mut self) -> Result<Vec<VibeMessage>, SessionError> {
        let mut out = Vec::new();
        while let Some(report) = self.transport.read_report()? {
            if let Ok(frame) = FrameCodec::decode_input_report(&report) {
                if self.verbose {
                    eprintln!("<< {}", frame.short_hex());
                }
                let msg = VibeMessage::parse(&frame);
                out.push(msg);
            }
        }
        Ok(out)
    }

    /// Sends a request, retrying every 100 ms until a matching reply arrives or timeout elapses.
    pub fn request(&mut self, req: &Request) -> Result<Frame, SessionError> {
        self.request_with_timeout(req, self.default_timeout)
    }

    pub fn request_with_timeout(
        &mut self,
        req: &Request,
        timeout: Duration,
    ) -> Result<Frame, SessionError> {
        let Some(ref matcher) = req.reply else {
            self.send_raw(&req.bytes)?;
            return Ok(Frame::new(req.bytes.to_vec()));
        };
        let deadline = Instant::now() + timeout;
        let mut next_send = Instant::now();

        while Instant::now() < deadline {
            self.maybe_send_heartbeat()?;
            if Instant::now() >= next_send {
                self.send_raw(&req.bytes)?;
                next_send = Instant::now() + self.retry_interval;
            }
            while let Some(report) = self.transport.read_report()? {
                if let Ok(frame) = FrameCodec::decode_input_report(&report) {
                    if self.verbose {
                        eprintln!("<< {}", frame.short_hex());
                    }
                    if matcher.matches(&frame) {
                        return Ok(frame);
                    }
                    let msg = VibeMessage::parse(&frame);
                    if !matches!(msg, VibeMessage::Heartbeat) {
                        self.event_queue.push_back(msg);
                    }
                }
            }
            thread::sleep(Duration::from_millis(8));
        }
        Err(SessionError::Timeout(req.name))
    }

    /// Collects multi-chunk replies (e.g. mic serial number and UUID).
    pub fn collect_chunks(
        &mut self,
        req: &Request,
        window: Duration,
    ) -> Result<Vec<Frame>, SessionError> {
        let first = self.request(req)?;
        let Some(ref matcher) = req.reply else {
            return Ok(vec![first]);
        };
        let mut frames = vec![first];
        let deadline = Instant::now() + window;
        while Instant::now() < deadline {
            while let Some(report) = self.transport.read_report()? {
                if let Ok(frame) = FrameCodec::decode_input_report(&report) {
                    if matcher.matches(&frame) {
                        frames.push(frame);
                    } else {
                        let msg = VibeMessage::parse(&frame);
                        if !matches!(msg, VibeMessage::Heartbeat) {
                            self.event_queue.push_back(msg);
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(8));
        }
        Ok(frames)
    }

    // --- Typed high-level operations ---

    pub fn is_mic_linked(&mut self) -> Result<bool, SessionError> {
        let f = self.request(&DongleRequest::device_active())?;
        match VibeMessage::parse(&f) {
            VibeMessage::DeviceActive(linked) => Ok(linked),
            _ => Ok(f.byte(4) != 0),
        }
    }

    pub fn read_battery(&mut self) -> Result<BatteryStatus, SessionError> {
        let f = self.request(&DeviceRequest::battery())?;
        match VibeMessage::parse(&f) {
            VibeMessage::Battery(b) => Ok(b),
            _ => Err(SessionError::Protocol("unexpected battery reply".into())),
        }
    }

    pub fn read_info(&mut self) -> Result<DeviceInfo, SessionError> {
        let mut info = DeviceInfo {
            hid_path: self.device_path().map(String::from),
            ..Default::default()
        };
        if let Ok(f) = self.request(&DongleRequest::version()) {
            if let VibeMessage::DongleVersion(v) = VibeMessage::parse(&f) {
                info.dongle_version = Some(v);
            }
        }
        if let Ok(f) = self.request(&DongleRequest::serial_number()) {
            if let VibeMessage::DongleSerial(s) = VibeMessage::parse(&f) {
                if !s.is_empty() {
                    info.dongle_serial = Some(s);
                }
            }
        }
        if let Ok(f) = self.request(&DongleRequest::flash_id()) {
            if let VibeMessage::DongleFlashId(id) = VibeMessage::parse(&f) {
                info.dongle_flash_id = Some(hex_string(&id));
            }
        }
        if let Ok(f) = self.request(&DongleRequest::charging_status()) {
            if let VibeMessage::DongleCharging(c) = VibeMessage::parse(&f) {
                info.dongle_charging = Some(c);
            }
        }
        info.mic_linked = self.is_mic_linked().unwrap_or(false);
        if info.mic_linked {
            if let Ok(f) = self.request(&DeviceRequest::version()) {
                if let VibeMessage::DeviceVersion(v) = VibeMessage::parse(&f) {
                    info.device_version = Some(v);
                }
            }
            if let Ok(f) = self.request(&DeviceRequest::hardware_version()) {
                if let VibeMessage::HardwareVersion(v) = VibeMessage::parse(&f) {
                    info.hardware_version = Some(v);
                }
            }
            if let Ok(f) = self.request(&DeviceRequest::flash_id()) {
                if let VibeMessage::DeviceFlashId(id) = VibeMessage::parse(&f) {
                    info.device_flash_id = Some(hex_string(&id));
                }
            }
            if let Ok(frames) =
                self.collect_chunks(&DeviceRequest::serial_number(), Duration::from_millis(250))
            {
                let mut chunks: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
                for f in frames {
                    if let VibeMessage::DeviceSerialChunk { offset, bytes } = VibeMessage::parse(&f)
                    {
                        chunks.entry(offset).or_insert(bytes);
                    }
                }
                let joined: Vec<u8> = chunks.into_values().flatten().collect();
                let s = String::from_utf8_lossy(&joined)
                    .trim_end_matches('\0')
                    .to_string();
                if !s.is_empty() {
                    info.device_serial = Some(s);
                }
            }
            if let Ok(frames) =
                self.collect_chunks(&DeviceRequest::uuid(), Duration::from_millis(250))
            {
                let mut chunks: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
                for f in frames {
                    if let VibeMessage::UuidChunk { offset, bytes } = VibeMessage::parse(&f) {
                        chunks.entry(offset).or_insert(bytes);
                    }
                }
                let joined: Vec<u8> = chunks.into_values().flatten().collect();
                let s = String::from_utf8_lossy(&joined)
                    .trim_end_matches('\0')
                    .to_string();
                if !s.is_empty() {
                    info.device_uuid = Some(s);
                }
            }
            info.battery = self.read_battery().ok();
        }
        Ok(info)
    }

    pub fn read_settings(&mut self) -> Result<DeviceSettings, SessionError> {
        if !self.is_mic_linked()? {
            return Err(SessionError::MicNotLinked);
        }
        let mut s = DeviceSettings::default();
        if let Ok(f) = self.request(&DeviceRequest::sleep_time()) {
            if let VibeMessage::SleepTime(v) = VibeMessage::parse(&f) {
                s.sleep_time_secs = Some(v);
            }
        }
        if let Ok(f) = self.request(&DeviceRequest::standby_status()) {
            if let VibeMessage::StandbyStatus(v) = VibeMessage::parse(&f) {
                s.standby_status = Some(v);
            }
        }
        if let Ok(f) = self.request(&DeviceRequest::standby_time()) {
            if let VibeMessage::StandbyTime(v) = VibeMessage::parse(&f) {
                s.standby_time_secs = Some(v);
            }
        }
        if let Ok(f) = self.request(&DeviceRequest::mic_enable()) {
            if let VibeMessage::MicEnable(v) = VibeMessage::parse(&f) {
                s.mic_enabled = Some(v);
            }
        }
        if let Ok(f) = self.request(&DeviceRequest::noise_reduction()) {
            if let VibeMessage::NoiseReduction { level, low, high } = VibeMessage::parse(&f) {
                s.noise_reduction = Some((level, low, high));
            }
        }
        if let Ok(f) = self.request(&DeviceRequest::motor_strength()) {
            if let VibeMessage::MotorStrength(v) = VibeMessage::parse(&f) {
                s.motor_strength = Some(v);
            }
        }
        if let Ok(f) = self.request(&DeviceRequest::audio_button_mode()) {
            if let VibeMessage::AudioButtonMode(v) = VibeMessage::parse(&f) {
                s.audio_button_mode = Some(v);
            }
        }
        if let Ok(f) = self.request(&DeviceRequest::hooks_mode()) {
            if let VibeMessage::HooksMode(v) = VibeMessage::parse(&f) {
                s.hooks_mode = Some(v);
            }
        }
        if let Ok(f) = self.request(&DeviceRequest::indicator_lights()) {
            if let VibeMessage::IndicatorLights(v) = VibeMessage::parse(&f) {
                s.indicator_lights = Some(v);
            }
        }
        Ok(s)
    }

    pub fn read_indicator_lights(&mut self) -> Result<IndicatorLights, SessionError> {
        let f = self.request(&DeviceRequest::indicator_lights())?;
        match VibeMessage::parse(&f) {
            VibeMessage::IndicatorLights(l) => Ok(l),
            _ => Err(SessionError::Protocol("unexpected LED reply".into())),
        }
    }

    pub fn read_buttons(&mut self) -> Result<Vec<ButtonBindingInfo>, SessionError> {
        if !self.is_mic_linked()? {
            return Err(SessionError::MicNotLinked);
        }
        let mut out = Vec::with_capacity(Control::ALL.len());
        for control in Control::ALL {
            let sf = self.request(&DeviceRequest::button_shortcut(control.index()))?;
            let shortcut = match VibeMessage::parse(&sf) {
                VibeMessage::ButtonShortcut { keys, .. } => keys,
                _ => Vec::new(),
            };
            let ff = self.request(&DeviceRequest::button_fixed_function(control.index()))?;
            let fixed = match VibeMessage::parse(&ff) {
                VibeMessage::ButtonFixedFunction { function, .. } => function,
                _ => control.factory_fixed_function(),
            };
            out.push(ButtonBindingInfo {
                control,
                shortcut_text: Hotkey::render(&shortcut),
                shortcut,
                fixed_function: fixed,
                fixed_function_name: FixedFunction::name_for_code(fixed),
            });
        }
        Ok(out)
    }

    pub fn set_sleep_time(&mut self, seconds: u32) -> Result<(), SessionError> {
        self.request(&DeviceRequest::set_sleep_time(seconds))?;
        Ok(())
    }

    pub fn set_standby_status(&mut self, on: bool) -> Result<(), SessionError> {
        self.request(&DeviceRequest::set_standby_status(on))?;
        Ok(())
    }

    pub fn set_standby_time(&mut self, seconds: u32) -> Result<(), SessionError> {
        self.request(&DeviceRequest::set_standby_time(seconds))?;
        Ok(())
    }

    pub fn set_mic_enable(&mut self, on: bool) -> Result<(), SessionError> {
        self.request(&DeviceRequest::set_mic_enable(on))?;
        Ok(())
    }

    pub fn set_noise_reduction(
        &mut self,
        level: u8,
        low: u16,
        high: u16,
    ) -> Result<(), SessionError> {
        self.request(&DeviceRequest::set_noise_reduction(level, low, high))?;
        Ok(())
    }

    pub fn set_motor_strength(&mut self, value: u16) -> Result<(), SessionError> {
        self.request(&DeviceRequest::set_motor_strength(value))?;
        Ok(())
    }

    pub fn set_audio_button_mode(&mut self, mode: u8) -> Result<(), SessionError> {
        self.request(&DeviceRequest::set_audio_button_mode(mode))?;
        Ok(())
    }

    pub fn set_hooks_mode(&mut self, on: bool) -> Result<(), SessionError> {
        self.request(&DeviceRequest::set_hooks_mode(on))?;
        Ok(())
    }

    pub fn set_light_mode(&mut self, mode: LightMode) -> Result<(), SessionError> {
        self.request(&DeviceRequest::set_light_mode(mode as u8))?;
        Ok(())
    }

    pub fn set_all_on_brightness(&mut self, level: u8) -> Result<(), SessionError> {
        self.request(&DeviceRequest::set_all_on_brightness(level.min(20)))?;
        Ok(())
    }

    pub fn set_led_field(
        &mut self,
        led: u8,
        field: LedField,
        value: u8,
    ) -> Result<(), SessionError> {
        self.request(&DeviceRequest::set_led_field(led, field, value))?;
        Ok(())
    }

    pub fn set_button_shortcut(
        &mut self,
        control: Control,
        keys: &[ShortcutKey],
    ) -> Result<(), SessionError> {
        self.request(&DeviceRequest::set_button_shortcut(control.index(), keys))?;
        Ok(())
    }

    pub fn set_button_fixed_function(
        &mut self,
        control: Control,
        function: u32,
    ) -> Result<(), SessionError> {
        // Clear any stored shortcut so the firmware executes the fixed function.
        self.request(&DeviceRequest::set_button_shortcut(control.index(), &[]))?;
        self.request(&DeviceRequest::set_button_fixed_function(
            control.index(),
            function,
        ))?;
        Ok(())
    }

    pub fn reset_buttons(&mut self) -> Result<(), SessionError> {
        for control in Control::ALL {
            self.set_button_fixed_function(control, control.factory_fixed_function())?;
        }
        Ok(())
    }

    pub fn reboot_dongle(&mut self, delay_ms: u32) -> Result<(), SessionError> {
        self.request(&DongleRequest::reboot(delay_ms))?;
        Ok(())
    }

    pub fn reboot_device(&mut self, delay_ms: u32) -> Result<(), SessionError> {
        self.request(&DeviceRequest::reboot(delay_ms))?;
        Ok(())
    }
}

fn pseudo_random_u32() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x1234_5678);
    let pid = u64::from(std::process::id());
    let mut x = nanos ^ (pid.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    (x as u32).max(1)
}
