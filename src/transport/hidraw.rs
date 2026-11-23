//! Native Linux `/dev/hidraw*` transport for the Ulanzi Vibe Key AU05.
//! Zero libhidapi/libudev C dependencies — talks directly to sysfs and hidraw.

use crate::protocol::{USB_PID, USB_VID};
use crate::transport::{is_au05_vendor_descriptor, DiscoveredDevice, Transport, TransportError};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// Discovers all connected AU05 vendor HID interfaces on `/sys/class/hidraw`.
pub fn discover_au05_devices() -> Result<Vec<DiscoveredDevice>, TransportError> {
    let entries = match fs::read_dir("/sys/class/hidraw") {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(TransportError::Io(e.to_string())),
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let dev_dir = entry.path().join("device");
        let Ok(uevent) = fs::read_to_string(dev_dir.join("uevent")) else {
            continue;
        };
        let mut hid_id_matches = false;
        let mut serial = None;
        let mut phys = None;
        let mut name = None;
        for line in uevent.lines() {
            if let Some(id) = line.strip_prefix("HID_ID=") {
                let parts: Vec<_> = id.split(':').collect();
                if parts.len() == 3
                    && u32::from_str_radix(parts[1], 16) == Ok(u32::from(USB_VID))
                    && u32::from_str_radix(parts[2], 16) == Ok(u32::from(USB_PID))
                {
                    hid_id_matches = true;
                }
            } else if let Some(v) = line.strip_prefix("HID_UNIQ=") {
                if !v.is_empty() {
                    serial = Some(v.to_string());
                }
            } else if let Some(v) = line.strip_prefix("HID_PHYS=") {
                if !v.is_empty() {
                    phys = Some(v.to_string());
                }
            } else if let Some(v) = line.strip_prefix("HID_NAME=") {
                if !v.is_empty() {
                    name = Some(v.to_string());
                }
            }
        }
        if !hid_id_matches {
            continue;
        }
        let Ok(descriptor) = fs::read(dev_dir.join("report_descriptor")) else {
            continue;
        };
        if is_au05_vendor_descriptor(&descriptor) {
            let path = Path::new("/dev")
                .join(entry.file_name())
                .to_string_lossy()
                .into_owned();
            found.push(DiscoveredDevice {
                path,
                serial,
                phys,
                name,
            });
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
}

#[derive(Default)]
pub struct HidrawTransport {
    device_override: Option<String>,
    file: Option<File>,
    path: Option<PathBuf>,
    path_str: Option<String>,
}

impl HidrawTransport {
    pub fn new(device_override: Option<String>) -> Self {
        Self {
            device_override,
            file: None,
            path: None,
            path_str: None,
        }
    }

    fn classify_io(path: &str, err: io::Error) -> TransportError {
        match err.raw_os_error() {
            Some(libc::EACCES | libc::EPERM) => TransportError::PermissionDenied {
                path: path.to_string(),
                detail: err.to_string(),
            },
            Some(libc::EBUSY | libc::EAGAIN) => TransportError::Busy {
                path: path.to_string(),
                detail: err.to_string(),
            },
            Some(libc::ENOENT | libc::ENODEV) => TransportError::NotFound(err.to_string()),
            _ => TransportError::Io(format!("{path}: {err}")),
        }
    }
}

impl Transport for HidrawTransport {
    fn connect(&mut self) -> Result<(), TransportError> {
        self.close();
        let target_path = if let Some(ref explicit) = self.device_override {
            explicit.clone()
        } else {
            let devices = discover_au05_devices()?;
            match devices.len() {
                0 => {
                    return Err(TransportError::NotFound(
                        "no AU05 vendor interface (fff1:00dd, usage page 0xFFFC) detected".into(),
                    ))
                }
                1 => devices[0].path.clone(),
                _ => {
                    return Err(TransportError::MultipleDevices(
                        devices.into_iter().map(|d| d.path).collect(),
                    ))
                }
            }
        };

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&target_path)
            .map_err(|e| Self::classify_io(&target_path, e))?;

        // Advisory exclusive lock so concurrent open-au05 instances do not collide.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(Self::classify_io(&target_path, io::Error::last_os_error()));
        }

        self.path = Some(PathBuf::from(&target_path));
        self.path_str = Some(target_path);
        self.file = Some(file);
        Ok(())
    }

    fn read_report(&mut self) -> Result<Option<Vec<u8>>, TransportError> {
        let Some(ref path) = self.path else {
            return Err(TransportError::NotFound("not connected".into()));
        };
        if !path.exists() {
            return Err(TransportError::NotFound("receiver disconnected".into()));
        }
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|e| TransportError::Io(e.to_string()))?;
        if unsafe { libc::access(c_path.as_ptr(), libc::R_OK | libc::W_OK) } != 0 {
            let path_s = self.path_str.as_deref().unwrap_or("/dev/hidraw?");
            return Err(Self::classify_io(path_s, io::Error::last_os_error()));
        }
        let mut buf = [0u8; 65];
        let path_s = self.path_str.as_deref().unwrap_or("/dev/hidraw?");
        match self.file.as_mut().unwrap().read(&mut buf) {
            Ok(0) => Err(TransportError::NotFound("receiver closed".into())),
            Ok(n) => Ok(Some(buf[..n].to_vec())),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(Self::classify_io(path_s, e)),
        }
    }

    fn write_report(&mut self, report: &[u8]) -> Result<(), TransportError> {
        let path_s = self.path_str.as_deref().unwrap_or("/dev/hidraw?");
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| TransportError::NotFound("not connected".into()))?;
        match file.write(report) {
            Ok(n) if n == report.len() => Ok(()),
            Ok(n) => Err(TransportError::Io(format!(
                "short HID write to {path_s}: {n}/{}",
                report.len()
            ))),
            Err(e) => Err(Self::classify_io(path_s, e)),
        }
    }

    fn close(&mut self) {
        self.file = None;
        self.path = None;
        self.path_str = None;
    }

    fn device_path(&self) -> Option<&str> {
        self.path_str.as_deref()
    }
}
