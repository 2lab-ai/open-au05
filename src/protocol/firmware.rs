//! AU05 firmware image parser, Ulanzi update check client, and OTA state machine.

use crate::protocol::messages::{UpgradeEvent, VibeMessage};
use crate::protocol::requests::UpgradeRequest;
use crate::protocol::types::MessageType;
use crate::session::Session;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FirmwareTarget {
    Dongle,
    Device,
}

impl FirmwareTarget {
    pub fn message_type(self) -> u8 {
        match self {
            Self::Dongle => MessageType::DongleUpgrade as u8,
            Self::Device => MessageType::DeviceUpgrade as u8,
        }
    }

    pub fn package_interval_ms(self) -> u64 {
        match self {
            Self::Dongle => 2,
            Self::Device => 10,
        }
    }

    pub fn connect_interval_ms(self) -> u64 {
        match self {
            Self::Dongle => 200,
            Self::Device => 500,
        }
    }

    pub fn api_product_id(self) -> &'static str {
        match self {
            Self::Dongle => "AU05_USB",
            Self::Device => "AU05_Device",
        }
    }
}

impl fmt::Display for FirmwareTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dongle => f.write_str("dongle"),
            Self::Device => f.write_str("device"),
        }
    }
}

impl FromStr for FirmwareTarget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dongle" | "rx" | "usb" => Ok(Self::Dongle),
            "device" | "mic" | "tx" => Ok(Self::Device),
            other => Err(format!(
                "invalid firmware target '{other}' (expected: dongle, device)"
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FirmwareError {
    TooSmall(usize),
    WrongTarget(u8, FirmwareTarget),
    Truncated { expected: usize, actual: usize },
    StepTimeout(String),
    ProgramFailed(usize),
    UpdateFailed(u8),
    Transport(String),
}

impl fmt::Display for FirmwareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooSmall(n) => {
                write!(f, "firmware image is {n} bytes (minimum is 10240 bytes)")
            }
            Self::WrongTarget(b, t) => {
                write!(f, "image header target {b} is not valid for {t}")
            }
            Self::Truncated { expected, actual } => {
                write!(
                    f,
                    "header declares {expected} payload bytes, but only {actual} available"
                )
            }
            Self::StepTimeout(step) => write!(f, "OTA update timed out at '{step}'"),
            Self::ProgramFailed(frame) => write!(f, "device failed to program frame {frame}"),
            Self::UpdateFailed(code) => write!(f, "device reported update failure code {code}"),
            Self::Transport(msg) => write!(f, "transport error: {msg}"),
        }
    }
}

impl std::error::Error for FirmwareError {}

#[derive(Clone, Debug)]
pub struct FirmwareImage {
    pub header: [u8; 32],
    pub payload: Vec<u8>,
    pub frames: Vec<Vec<u8>>,
}

impl FirmwareImage {
    pub const HEADER_LENGTH: usize = 32;
    pub const FRAME_LENGTH: usize = 1024;
    pub const PACKAGE_LENGTH: usize = 32;

    pub fn parse(bytes: &[u8], target: FirmwareTarget) -> Result<Self, FirmwareError> {
        if (bytes.len() >> 11) < 5 {
            return Err(FirmwareError::TooSmall(bytes.len()));
        }
        let target_byte = bytes[4];
        match target {
            FirmwareTarget::Device if target_byte != 1 && target_byte != 2 => {
                return Err(FirmwareError::WrongTarget(target_byte, target));
            }
            FirmwareTarget::Dongle if (target_byte & 0xFD) != 0 => {
                return Err(FirmwareError::WrongTarget(target_byte, target));
            }
            _ => {}
        }
        let mut header = [0u8; Self::HEADER_LENGTH];
        header.copy_from_slice(&bytes[..Self::HEADER_LENGTH]);
        let declared_len =
            u32::from_le_bytes([header[28], header[29], header[30], header[31]]) as usize;
        let available = bytes.len() - Self::HEADER_LENGTH;
        if declared_len > available {
            return Err(FirmwareError::Truncated {
                expected: declared_len,
                actual: available,
            });
        }
        let payload = bytes[Self::HEADER_LENGTH..Self::HEADER_LENGTH + declared_len].to_vec();
        let frames = payload
            .chunks(Self::FRAME_LENGTH)
            .map(|c| c.to_vec())
            .collect();
        Ok(Self {
            header,
            payload,
            frames,
        })
    }

    pub fn custom_code(&self) -> &[u8] {
        &self.header[5..8]
    }

    pub fn payload_length(&self) -> u32 {
        self.payload.len() as u32
    }

    pub fn checksum(bytes: &[u8]) -> u32 {
        bytes
            .iter()
            .fold(0u32, |acc, &b| acc.wrapping_add(u32::from(b)))
    }

    pub fn total_checksum(&self) -> u32 {
        self.frames
            .iter()
            .fold(0u32, |acc, f| acc.wrapping_add(Self::checksum(f)))
    }

    pub fn packages(frame: &[u8]) -> Vec<&[u8]> {
        frame.chunks(Self::PACKAGE_LENGTH).collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FirmwareUpdateInfo {
    pub target: FirmwareTarget,
    pub current_version: String,
    pub need_update: bool,
    pub version: Option<String>,
    pub download_url: Option<String>,
    pub notes: Option<String>,
    pub raw: String,
}

impl FirmwareUpdateInfo {
    pub const API_BASE: &'static str = "https://api.ulanzistudio.com/api";

    pub fn check(
        serial: &str,
        target: FirmwareTarget,
        current_version: &str,
        lang: &str,
    ) -> Result<Self, String> {
        let url = format!("{}/vibekey/firmware/checkUpdate", Self::API_BASE);
        let resp = ureq::get(&url)
            .query("deviceSn", serial)
            .query("pid", target.api_product_id())
            .query("ver", current_version)
            .query("lang", lang)
            .timeout(Duration::from_secs(10))
            .call()
            .map_err(|e| format!("firmware check request failed: {e}"))?;
        let raw = resp
            .into_string()
            .map_err(|e| format!("failed to read firmware check response: {e}"))?;
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
        let d = json.get("data").unwrap_or(&json);
        let need_update = match d.get("needUpdate") {
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
            Some(serde_json::Value::String(s)) => s == "1" || s.eq_ignore_ascii_case("true"),
            _ => false,
        };
        Ok(Self {
            target,
            current_version: current_version.to_string(),
            need_update,
            version: d.get("version").and_then(|v| v.as_str()).map(String::from),
            download_url: d
                .get("downloadUrl")
                .and_then(|v| v.as_str())
                .map(String::from),
            notes: d
                .get("versionInfo")
                .and_then(|v| v.as_str())
                .map(String::from),
            raw,
        })
    }
}

pub struct FirmwareUpdater {
    pub step_timeout: Duration,
}

impl Default for FirmwareUpdater {
    fn default() -> Self {
        Self {
            step_timeout: Duration::from_secs(10),
        }
    }
}

impl FirmwareUpdater {
    pub fn run<F>(
        &self,
        session: &mut Session,
        image: &FirmwareImage,
        target: FirmwareTarget,
        mut on_progress: F,
    ) -> Result<(), FirmwareError>
    where
        F: FnMut(f64, &str),
    {
        let msg_type = target.message_type();
        session.set_heartbeat_enabled(false);
        let res = self.run_inner(session, image, target, msg_type, &mut on_progress);
        session.set_heartbeat_enabled(true);
        res
    }

    fn run_inner<F>(
        &self,
        session: &mut Session,
        image: &FirmwareImage,
        target: FirmwareTarget,
        msg_type: u8,
        on_progress: &mut F,
    ) -> Result<(), FirmwareError>
    where
        F: FnMut(f64, &str),
    {
        on_progress(0.0, "connecting");
        let connect =
            UpgradeRequest::connect(msg_type, image.payload_length(), image.custom_code());
        self.exchange(
            session,
            msg_type,
            &connect,
            Duration::from_millis(target.connect_interval_ms()),
            "connect",
            |e| matches!(e, UpgradeEvent::Connected),
        )?;

        let mut frame_index = 0usize;
        while frame_index < image.frames.len() {
            let frame = &image.frames[frame_index];
            let packages = FirmwareImage::packages(frame);
            let checksum = FirmwareImage::checksum(frame);
            let mut to_send: Vec<usize> = (0..packages.len()).collect();
            let mut attempts = 0usize;

            loop {
                attempts += 1;
                if attempts > 20 {
                    return Err(FirmwareError::StepTimeout(format!(
                        "frame {frame_index} transfer"
                    )));
                }
                for &pkg_idx in &to_send {
                    let pkt = UpgradeRequest::data(msg_type, pkg_idx as u16, packages[pkg_idx]);
                    session
                        .send_raw(&pkt)
                        .map_err(|e| FirmwareError::Transport(e.to_string()))?;
                    thread::sleep(Duration::from_millis(target.package_interval_ms()));
                }
                let req = UpgradeRequest::receive_package_num(msg_type, checksum);
                let reply = self.exchange(
                    session,
                    msg_type,
                    &req,
                    Duration::from_millis(target.package_interval_ms() + 45),
                    "receive package count",
                    |e| matches!(e, UpgradeEvent::ReceivePackageNum { .. }),
                )?;
                if let UpgradeEvent::ReceivePackageNum {
                    mask,
                    checksum: dev_sum,
                } = reply
                {
                    let missing: Vec<usize> = (0..packages.len())
                        .filter(|&i| i < 32 && (mask & (1u32 << i)) == 0)
                        .collect();
                    if !missing.is_empty() {
                        to_send = missing;
                        continue;
                    }
                    if dev_sum != checksum {
                        to_send = (0..packages.len()).collect();
                        continue;
                    }
                    break;
                }
            }

            let prog_interval =
                target.package_interval_ms() + if (packages.len() & 3) != 0 { 50 } else { 60 };
            let req = UpgradeRequest::enable_program(
                msg_type,
                frame_index as u16,
                packages.len() as u8,
                true,
            );
            let done = self.exchange(
                session,
                msg_type,
                &req,
                Duration::from_millis(prog_interval),
                &format!("program frame {frame_index}"),
                |e| matches!(e, UpgradeEvent::ProgramComplete { .. }),
            )?;
            if let UpgradeEvent::ProgramComplete { result, index, .. } = done {
                if result == 0 {
                    return Err(FirmwareError::ProgramFailed(frame_index));
                }
                frame_index = if usize::from(index) == frame_index {
                    frame_index + 1
                } else {
                    usize::from(index)
                };
                on_progress(
                    frame_index as f64 / image.frames.len() as f64,
                    &format!("frame {frame_index}/{}", image.frames.len()),
                );
            }
        }

        on_progress(1.0, "verifying checksum");
        let sum_reply = self.exchange(
            session,
            msg_type,
            &UpgradeRequest::check_all_sum(msg_type),
            Duration::from_millis(50),
            "check sum",
            |e| matches!(e, UpgradeEvent::CheckAllSum(_)),
        )?;
        let device_total = match sum_reply {
            UpgradeEvent::CheckAllSum(v) => v,
            _ => return Err(FirmwareError::StepTimeout("check sum".into())),
        };
        let matched = device_total == image.total_checksum();
        let final_reply = self.exchange(
            session,
            msg_type,
            &UpgradeRequest::all_page_complete_result(msg_type, matched),
            Duration::from_millis(100),
            "finish",
            |e| matches!(e, UpgradeEvent::Result(_)),
        )?;
        match final_reply {
            UpgradeEvent::Result(r) if r != 0 && matched => {
                on_progress(1.0, "done");
                Ok(())
            }
            UpgradeEvent::Result(r) => Err(FirmwareError::UpdateFailed(r)),
            _ => Err(FirmwareError::UpdateFailed(0)),
        }
    }

    fn exchange<P>(
        &self,
        session: &mut Session,
        msg_type: u8,
        bytes: &[u8; 64],
        resend_every: Duration,
        step: &str,
        predicate: P,
    ) -> Result<UpgradeEvent, FirmwareError>
    where
        P: Fn(&UpgradeEvent) -> bool,
    {
        let deadline = Instant::now() + self.step_timeout;
        let mut next_send = Instant::now();
        while Instant::now() < deadline {
            if Instant::now() >= next_send {
                session
                    .send_raw(bytes)
                    .map_err(|e| FirmwareError::Transport(e.to_string()))?;
                next_send = Instant::now() + resend_every.max(Duration::from_millis(5));
            }
            let msgs = session
                .poll_raw_messages()
                .map_err(|e| FirmwareError::Transport(e.to_string()))?;
            for msg in msgs {
                if let VibeMessage::Upgrade { target, event } = msg {
                    if target == msg_type && predicate(&event) {
                        return Ok(event);
                    }
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        Err(FirmwareError::StepTimeout(step.to_string()))
    }
}
