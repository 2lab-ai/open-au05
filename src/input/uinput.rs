//! Pure-Rust Linux `/dev/uinput` virtual keyboard + mouse + media key emitter.
//! Works identically across Wayland (KDE Plasma, Hyprland, Sway, Niri, GNOME) and X11.

use crate::input::{fixed_function_to_linux_key, hid_to_linux_key, InputEmitter, InputError};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const SYN_REPORT: u16 = 0x00;
const REL_WHEEL: u16 = 0x08;

// Linux uinput ioctl numbers (stable kernel ABI)
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x5502;
const UI_DEV_SETUP: libc::c_ulong = 0x405c_5503;
const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_SET_RELBIT: libc::c_ulong = 0x4004_5566;

#[derive(Default)]
pub struct UInputEmitter {
    file: Option<File>,
    ready_at: Option<Instant>,
}

impl UInputEmitter {
    fn create_device() -> io::Result<File> {
        let file = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open("/dev/uinput")?;

        let set_bit = |req: libc::c_ulong, val: i32| -> io::Result<()> {
            if unsafe { libc::ioctl(file.as_raw_fd(), req, val) } < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        };

        set_bit(UI_SET_EVBIT, i32::from(EV_KEY))?;
        set_bit(UI_SET_EVBIT, i32::from(EV_REL))?;
        set_bit(UI_SET_RELBIT, i32::from(REL_WHEEL))?;

        // Enable all mapped keyboard, modifier, function, media, and mouse buttons.
        for hid in (0x04..=0x81).chain(0xE0..=0xE7).chain(0x100..=0x102) {
            if let Some(code) = hid_to_linux_key(hid) {
                set_bit(UI_SET_KEYBIT, i32::from(code))?;
            }
        }
        for fixed in [7u32, 8, 9, 11, 12, 13] {
            if let Some(code) = fixed_function_to_linux_key(fixed) {
                set_bit(UI_SET_KEYBIT, i32::from(code))?;
            }
        }

        // struct uinput_setup (92 bytes):
        //   input_id id (8 bytes: bustype=BUS_VIRTUAL(6), vendor=0xfff1, product=0x00dd, version=1)
        //   char name[80]
        //   u32 ff_effects_max
        let mut setup = [0u8; 92];
        setup[0..2].copy_from_slice(&6u16.to_ne_bytes());
        setup[2..4].copy_from_slice(&0xFFF1u16.to_ne_bytes());
        setup[4..6].copy_from_slice(&0x00DDu16.to_ne_bytes());
        setup[6..8].copy_from_slice(&1u16.to_ne_bytes());
        let name = b"open-au05 virtual input";
        setup[8..8 + name.len()].copy_from_slice(name);

        if unsafe { libc::ioctl(file.as_raw_fd(), UI_DEV_SETUP, setup.as_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::ioctl(file.as_raw_fd(), UI_DEV_CREATE) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(file)
    }

    fn write_event(&mut self, kind: u16, code: u16, value: i32) -> Result<(), InputError> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| InputError::Io("uinput device not initialized".into()))?;
        let mut buf = Vec::with_capacity((std::mem::size_of::<libc::timeval>() + 8) * 2);
        append_event(&mut buf, kind, code, value);
        append_event(&mut buf, EV_SYN, SYN_REPORT, 0);
        file.write_all(&buf)
            .map_err(|e| InputError::Io(e.to_string()))
    }
}

fn append_event(buf: &mut Vec<u8>, kind: u16, code: u16, value: i32) {
    buf.resize(buf.len() + std::mem::size_of::<libc::timeval>(), 0);
    buf.extend_from_slice(&kind.to_ne_bytes());
    buf.extend_from_slice(&code.to_ne_bytes());
    buf.extend_from_slice(&value.to_ne_bytes());
}

impl InputEmitter for UInputEmitter {
    fn ensure(&mut self) -> Result<(), InputError> {
        if unsafe { libc::access(c"/dev/uinput".as_ptr(), libc::W_OK) } != 0 {
            return Err(InputError::PermissionDenied(
                io::Error::last_os_error().to_string(),
            ));
        }
        if self.file.is_none() {
            let file =
                Self::create_device().map_err(|e| InputError::PermissionDenied(e.to_string()))?;
            self.file = Some(file);
            self.ready_at = Some(Instant::now() + Duration::from_millis(350));
        }
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.file.is_some() && self.ready_at.is_some_and(|at| Instant::now() >= at)
    }

    fn emit_hid(&mut self, hid_code: u16, down: bool) -> Result<(), InputError> {
        // Mouse wheel scroll codes emit REL_WHEEL on press only.
        if hid_code == 0x105 {
            return if down {
                self.write_event(EV_REL, REL_WHEEL, 1)
            } else {
                Ok(())
            };
        }
        if hid_code == 0x106 {
            return if down {
                self.write_event(EV_REL, REL_WHEEL, -1)
            } else {
                Ok(())
            };
        }
        let code = hid_to_linux_key(hid_code).ok_or(InputError::UnsupportedKey(hid_code))?;
        self.write_event(EV_KEY, code, i32::from(down))
    }

    fn emit_fixed_function(&mut self, function: u32, down: bool) -> Result<(), InputError> {
        if let Some(code) = fixed_function_to_linux_key(function) {
            return self.write_event(EV_KEY, code, i32::from(down));
        }
        Ok(())
    }

    fn close(&mut self) {
        self.ready_at = None;
        if let Some(file) = self.file.take() {
            unsafe {
                libc::ioctl(file.as_raw_fd(), UI_DEV_DESTROY);
            }
        }
    }
}

impl Drop for UInputEmitter {
    fn drop(&mut self) {
        self.close();
    }
}
