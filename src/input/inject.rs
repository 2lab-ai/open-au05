//! Reliable Unicode / Korean / English text injection for Linux Wayland and X11.

use crate::config::{InjectMethod, PasteShortcut};
use crate::input::scheduler::KeyScheduler;
use crate::input::InputEmitter;
use crate::protocol::Hotkey;
use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct TextInjectorConfig {
    pub method: InjectMethod,
    pub paste_shortcut: PasteShortcut,
    pub restore_clipboard: bool,
    pub auto_enter: bool,
}

impl Default for TextInjectorConfig {
    fn default() -> Self {
        Self {
            method: InjectMethod::ClipboardPaste,
            paste_shortcut: PasteShortcut::CtrlShiftV,
            restore_clipboard: true,
            auto_enter: false,
        }
    }
}

pub fn inject_text(
    text: &str,
    cfg: &TextInjectorConfig,
    emitter: Option<&mut dyn InputEmitter>,
) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }
    match cfg.method {
        InjectMethod::ClipboardOnly => write_clipboard(text),
        InjectMethod::Wtype => {
            let status = Command::new("wtype")
                .arg("--")
                .arg(text)
                .status()
                .map_err(|e| format!("failed to run wtype: {e}"))?;
            if !status.success() {
                return Err(format!("wtype exited with {status}"));
            }
            if cfg.auto_enter {
                let _ = Command::new("wtype").args(["-k", "Return"]).status();
            }
            Ok(())
        }
        InjectMethod::Xdotool => {
            let status = Command::new("xdotool")
                .args(["type", "--clearmodifiers", "--", text])
                .status()
                .map_err(|e| format!("failed to run xdotool: {e}"))?;
            if !status.success() {
                return Err(format!("xdotool exited with {status}"));
            }
            if cfg.auto_enter {
                let _ = Command::new("xdotool").args(["key", "Return"]).status();
            }
            Ok(())
        }
        InjectMethod::ClipboardPaste => {
            let saved = if cfg.restore_clipboard {
                read_clipboard().ok()
            } else {
                None
            };

            write_clipboard(text)?;
            thread::sleep(Duration::from_millis(45));

            let paste_hotkey = match cfg.paste_shortcut {
                PasteShortcut::CtrlV => "ctrl+v",
                PasteShortcut::CtrlShiftV => "ctrl+shift+v",
                PasteShortcut::ShiftInsert => "shift+insert",
            };
            let codes = Hotkey::parse_hid_codes(paste_hotkey).map_err(|e| e.to_string())?;
            if let Some(em) = emitter {
                em.ensure().map_err(|e| e.to_string())?;
                while !em.is_ready() {
                    thread::sleep(Duration::from_millis(20));
                }
                let mut sched = KeyScheduler::default();
                sched.tap_combo(&codes, em).map_err(|e| e.to_string())?;
                if cfg.auto_enter {
                    thread::sleep(Duration::from_millis(35));
                    sched.tap_combo(&[0x28], em).map_err(|e| e.to_string())?;
                }
            } else {
                return Err(
                    "clipboard updated, but no uinput emitter available to send paste shortcut"
                        .into(),
                );
            }

            if let Some(old_bytes) = saved {
                thread::sleep(Duration::from_millis(160));
                let _ = write_clipboard_bytes(&old_bytes);
            }
            Ok(())
        }
    }
}

pub fn read_clipboard() -> Result<Vec<u8>, String> {
    if command_exists("wl-paste") {
        let out = Command::new("wl-paste")
            .arg("--no-newline")
            .output()
            .map_err(|e| format!("wl-paste failed: {e}"))?;
        if out.status.success() {
            return Ok(out.stdout);
        }
    }
    if command_exists("xclip") {
        let out = Command::new("xclip")
            .args(["-o", "-selection", "clipboard"])
            .output()
            .map_err(|e| format!("xclip failed: {e}"))?;
        if out.status.success() {
            return Ok(out.stdout);
        }
    }
    if command_exists("xsel") {
        let out = Command::new("xsel")
            .args(["-ob"])
            .output()
            .map_err(|e| format!("xsel failed: {e}"))?;
        if out.status.success() {
            return Ok(out.stdout);
        }
    }
    Err("no clipboard reader found (install wl-clipboard or xclip)".into())
}

pub fn write_clipboard(text: &str) -> Result<(), String> {
    write_clipboard_bytes(text.as_bytes())
}

fn write_clipboard_bytes(bytes: &[u8]) -> Result<(), String> {
    if command_exists("wl-copy") {
        let mut child = Command::new("wl-copy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to start wl-copy: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(bytes)
                .map_err(|e| format!("wl-copy stdin error: {e}"))?;
        }
        let status = child
            .wait()
            .map_err(|e| format!("wl-copy wait error: {e}"))?;
        if status.success() {
            return Ok(());
        }
    }
    if command_exists("xclip") {
        let mut child = Command::new("xclip")
            .args(["-i", "-selection", "clipboard"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to start xclip: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(bytes)
                .map_err(|e| format!("xclip stdin error: {e}"))?;
        }
        let status = child.wait().map_err(|e| format!("xclip wait error: {e}"))?;
        if status.success() {
            return Ok(());
        }
    }
    Err("no clipboard writer found (install wl-clipboard for Wayland or xclip for X11)".into())
}

pub fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let full = dir.join(name);
                full.is_file()
            })
        })
        .unwrap_or(false)
}
