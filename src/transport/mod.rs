//! HID transport abstraction for the AU05 vendor channel (`fff1:00dd`, interface 3).

#[cfg(target_os = "linux")]
pub mod hidraw;
pub mod mock;

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    NotFound(String),
    MultipleDevices(Vec<String>),
    PermissionDenied { path: String, detail: String },
    Busy { path: String, detail: String },
    Io(String),
    UnsupportedPlatform,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(msg) => write!(f, "AU05 device not found: {msg}"),
            Self::MultipleDevices(paths) => write!(
                f,
                "multiple AU05 receivers found ({}); pass --device or disconnect extras",
                paths.join(", ")
            ),
            Self::PermissionDenied { path, detail } => write!(
                f,
                "permission denied opening {path} ({detail}); run `open-au05 setup udev` and replug the AU05 receiver"
            ),
            Self::Busy { path, detail } => write!(
                f,
                "AU05 receiver {path} is busy ({detail}); close other AU05 clients or stop the daemon first"
            ),
            Self::Io(msg) => write!(f, "HID I/O error: {msg}"),
            Self::UnsupportedPlatform => {
                f.write_str("native hidraw transport is only available on Linux")
            }
        }
    }
}

impl std::error::Error for TransportError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredDevice {
    pub path: String,
    pub serial: Option<String>,
    pub phys: Option<String>,
    pub name: Option<String>,
}

pub trait Transport: Send {
    fn connect(&mut self) -> Result<(), TransportError>;
    fn read_report(&mut self) -> Result<Option<Vec<u8>>, TransportError>;
    fn write_report(&mut self, report: &[u8]) -> Result<(), TransportError>;
    fn close(&mut self);
    fn device_path(&self) -> Option<&str>;
}

/// Checks whether a USB HID report descriptor exposes the AU05 vendor channel
/// (usage page `0xFFFC`, report ID `0x55`, 63-byte input and output reports).
pub fn is_au05_vendor_descriptor(bytes: &[u8]) -> bool {
    let mut offset = 0usize;
    let (mut page, mut report, mut size, mut count) = (0u32, 0u32, 0u32, 0u32);
    let (mut has_input, mut has_output) = (false, false);
    let mut stack = Vec::new();
    while offset < bytes.len() {
        let prefix = bytes[offset];
        offset += 1;
        if prefix == 0xFE {
            if offset + 2 > bytes.len() {
                return false;
            }
            let n = usize::from(bytes[offset]);
            offset += 2 + n;
            if offset > bytes.len() {
                return false;
            }
            continue;
        }
        let n = match prefix & 3 {
            3 => 4usize,
            x => usize::from(x),
        };
        if offset + n > bytes.len() {
            return false;
        }
        let mut value = 0u32;
        for (i, &b) in bytes[offset..offset + n].iter().enumerate() {
            value |= u32::from(b) << (8 * i);
        }
        offset += n;
        match prefix & 0xFC {
            0x04 => page = value,
            0x84 => report = value,
            0x74 => size = value,
            0x94 => count = value,
            0xA4 => stack.push((page, report, size, count)),
            0xB4 => match stack.pop() {
                Some(s) => (page, report, size, count) = s,
                None => return false,
            },
            0x80 if page == 0xFFFC && report == 0x55 && size == 8 && count == 63 => {
                has_input = true
            }
            0x90 if page == 0xFFFC && report == 0x55 && size == 8 && count == 63 => {
                has_output = true
            }
            _ => {}
        }
    }
    has_input && has_output
}

pub fn open_default_transport(
    device_override: Option<String>,
) -> Result<Box<dyn Transport>, TransportError> {
    #[cfg(target_os = "linux")]
    {
        let mut t = hidraw::HidrawTransport::new(device_override);
        t.connect()?;
        Ok(Box::new(t))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = device_override;
        Err(TransportError::UnsupportedPlatform)
    }
}
