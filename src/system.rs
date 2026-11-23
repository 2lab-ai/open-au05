//! Linux system integration: `doctor` diagnostics, udev rule generation,
//! and systemd user service management.

use crate::config::{Config, SttBackend};
use crate::control;
use crate::dictation::audio::{discover_au05_source, resolve_recorder};
use crate::hooks::{self, AGENTS};
use crate::input::inject::command_exists;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const UDEV_RULE_CONTENT: &str = include_str!("../packaging/70-open-au05.rules");
pub const MODULES_LOAD_CONTENT: &str = include_str!("../packaging/open-au05.conf");

#[derive(Clone, Debug, Serialize)]
pub struct CheckItem {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub fix: Option<String>,
}

pub fn run_doctor(config_path: Option<&Path>) -> Vec<CheckItem> {
    let mut items = Vec::new();
    let cfg = Config::load(config_path).unwrap_or_default();

    // 1. Platform & HID vendor interface
    #[cfg(target_os = "linux")]
    {
        match crate::transport::hidraw::discover_au05_devices() {
            Ok(devs) if devs.is_empty() => items.push(CheckItem {
                name: "AU05 USB receiver (fff1:00dd)".into(),
                ok: false,
                detail: "no AU05 vendor HID interface (usage page 0xFFFC) detected".into(),
                fix: Some("Plug in the AU05 USB receiver dongle and ensure it shows up in `lsusb -d fff1:00dd`.".into()),
            }),
            Ok(devs) => {
                let first = &devs[0];
                let rw = unsafe {
                    let c = std::ffi::CString::new(first.path.as_str()).unwrap();
                    libc::access(c.as_ptr(), libc::R_OK | libc::W_OK) == 0
                };
                items.push(CheckItem {
                    name: "AU05 USB receiver (fff1:00dd)".into(),
                    ok: true,
                    detail: format!(
                        "found {} (serial: {}, phys: {})",
                        first.path,
                        first.serial.as_deref().unwrap_or("n/a"),
                        first.phys.as_deref().unwrap_or("n/a")
                    ),
                    fix: None,
                });
                items.push(CheckItem {
                    name: format!("HIDRAW read/write access ({})", first.path),
                    ok: rw,
                    detail: if rw {
                        "accessible by current user".into()
                    } else {
                        "permission denied (owned by root without uaccess ACL)".into()
                    },
                    fix: if rw {
                        None
                    } else {
                        Some("Run `open-au05 setup udev --install` (or copy rules with sudo) then replug the AU05 dongle.".into())
                    },
                });
            }
            Err(e) => items.push(CheckItem {
                name: "AU05 USB receiver (fff1:00dd)".into(),
                ok: false,
                detail: e.to_string(),
                fix: None,
            }),
        }

        // 2. /dev/uinput
        let uinput_exists = Path::new("/dev/uinput").exists();
        let uinput_rw =
            uinput_exists && unsafe { libc::access(c"/dev/uinput".as_ptr(), libc::W_OK) == 0 };
        items.push(CheckItem {
            name: "Virtual input (/dev/uinput)".into(),
            ok: uinput_rw,
            detail: if uinput_rw {
                "writable by current user".into()
            } else if uinput_exists {
                "/dev/uinput exists but is not writable".into()
            } else {
                "/dev/uinput not loaded".into()
            },
            fix: if uinput_rw {
                None
            } else {
                Some("Run `open-au05 setup udev` to install uinput module & uaccess rule.".into())
            },
        });

        // 3. udev rule installed
        let rule_exists = Path::new("/etc/udev/rules.d/70-open-au05.rules").exists();
        items.push(CheckItem {
            name: "udev rule (/etc/udev/rules.d/70-open-au05.rules)".into(),
            ok: rule_exists,
            detail: if rule_exists {
                "installed".into()
            } else {
                "not installed".into()
            },
            fix: if rule_exists {
                None
            } else {
                Some("Run `open-au05 setup udev` to view or install the rule.".into())
            },
        });
    }

    #[cfg(not(target_os = "linux"))]
    {
        items.push(CheckItem {
            name: "Linux kernel HID/uinput".into(),
            ok: false,
            detail: format!(
                "running on {} (Linux required for live hardware)",
                std::env::consts::OS
            ),
            fix: None,
        });
    }

    // 4. PipeWire/PulseAudio AU05 microphone
    if let Some(src) = discover_au05_source() {
        items.push(CheckItem {
            name: "AU05 Microphone (PipeWire/PulseAudio)".into(),
            ok: true,
            detail: format!("{} ({})", src.name, src.backend),
            fix: None,
        });
    } else {
        items.push(CheckItem {
            name: "AU05 Microphone (PipeWire/PulseAudio)".into(),
            ok: false,
            detail: "AU05 audio source not detected via `pactl list sources short`".into(),
            fix: Some(
                "Ensure the AU05 USB receiver is plugged in and PipeWire/PulseAudio is running."
                    .into(),
            ),
        });
    }

    // 5. Audio recorder
    match resolve_recorder(cfg.dictation.recorder) {
        Ok(rec) => items.push(CheckItem {
            name: "Audio recorder".into(),
            ok: true,
            detail: format!("{rec:?}"),
            fix: None,
        }),
        Err(e) => items.push(CheckItem {
            name: "Audio recorder".into(),
            ok: false,
            detail: e,
            fix: Some(
                "Install pipewire (pw-record), pulseaudio-utils (parecord), or ffmpeg.".into(),
            ),
        }),
    }

    // 6. Clipboard / typing tools
    let has_wl = command_exists("wl-copy") && command_exists("wl-paste");
    let has_xclip = command_exists("xclip");
    let has_wtype = command_exists("wtype");
    items.push(CheckItem {
        name: "Text injection tools".into(),
        ok: has_wl || has_xclip || has_wtype,
        detail: format!(
            "wl-clipboard={}, xclip={}, wtype={}",
            yes_no(has_wl),
            yes_no(has_xclip),
            yes_no(has_wtype)
        ),
        fix: if has_wl || has_xclip || has_wtype {
            None
        } else {
            Some("Install `wl-clipboard` (Wayland) or `xclip` (X11).".into())
        },
    });

    // 7. STT backend readiness
    let (stt_ok, stt_detail, stt_fix) = match cfg.dictation.stt_backend {
        SttBackend::Openai => {
            let is_local = cfg.dictation.api_url.contains("127.0.0.1")
                || cfg.dictation.api_url.contains("localhost");
            let has_key = !cfg.dictation.api_key_env.is_empty()
                && std::env::var(&cfg.dictation.api_key_env)
                    .map(|v| !v.trim().is_empty())
                    .unwrap_or(false);
            if is_local || has_key {
                (
                    true,
                    format!(
                        "openai-compatible ({}, model={})",
                        cfg.dictation.api_url, cfg.dictation.model
                    ),
                    None,
                )
            } else {
                (
                    false,
                    format!(
                        "env {} is not set for {}",
                        cfg.dictation.api_key_env, cfg.dictation.api_url
                    ),
                    Some(format!(
                        "Export {}=... or set `stt_backend = \"command\"` / local Whisper URL in {}",
                        cfg.dictation.api_key_env,
                        Config::default_path().display()
                    )),
                )
            }
        }
        SttBackend::Command => {
            let cmd = cfg.dictation.command.as_deref().unwrap_or("");
            let bin = cmd.split_whitespace().next().unwrap_or("");
            let ok = !bin.is_empty() && (Path::new(bin).exists() || command_exists(bin));
            (
                ok,
                format!("local command (`{cmd}`)"),
                if ok {
                    None
                } else {
                    Some(format!(
                        "Install `{bin}` or update `dictation.command` in {}",
                        Config::default_path().display()
                    ))
                },
            )
        }
    };
    items.push(CheckItem {
        name: "Speech-to-Text backend".into(),
        ok: stt_ok,
        detail: stt_detail,
        fix: stt_fix,
    });

    // 8. Daemon running
    let daemon_running = control::send_command(&serde_json::json!({"cmd": "status"})).is_ok();
    items.push(CheckItem {
        name: "Background daemon".into(),
        ok: daemon_running,
        detail: if daemon_running {
            format!("running (socket {})", control::socket_path().display())
        } else {
            "not running".into()
        },
        fix: if daemon_running {
            None
        } else {
            Some("Start with `open-au05 daemon` or install via `open-au05 service install`.".into())
        },
    });

    // 9. AI coding agent hooks summary
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()));
    let mut installed_agents = Vec::new();
    for agent in AGENTS {
        let (n, total, _) = hooks::hook_status(agent, &home);
        if n > 0 {
            installed_agents.push(format!("{} ({n}/{total})", agent.id));
        }
    }
    items.push(CheckItem {
        name: "AI agent LED hooks".into(),
        ok: !installed_agents.is_empty(),
        detail: if installed_agents.is_empty() {
            "none installed (optional)".into()
        } else {
            installed_agents.join(", ")
        },
        fix: if installed_agents.is_empty() {
            Some("Optional: run `open-au05 hooks install` to drive AU05 LEDs from Claude Code / Codex / Gemini CLI.".into())
        } else {
            None
        },
    });

    items
}

pub fn print_udev_setup_instructions() {
    println!("# 1. Write /etc/udev/rules.d/70-open-au05.rules:");
    println!("sudo tee /etc/udev/rules.d/70-open-au05.rules <<'EOF'");
    print!("{UDEV_RULE_CONTENT}");
    println!("EOF");
    println!();
    println!("# 2. Ensure uinput loads at boot:");
    println!("sudo tee /etc/modules-load.d/open-au05.conf <<'EOF'");
    print!("{MODULES_LOAD_CONTENT}");
    println!("EOF");
    println!();
    println!("# 3. Load uinput & reload udev rules (then replug the AU05 USB receiver):");
    println!("sudo modprobe uinput && sudo udevadm control --reload-rules && sudo udevadm trigger");
}

pub fn install_udev_rules() -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(
            "root privileges are required to write /etc/udev/rules.d/70-open-au05.rules. Run with `sudo open-au05 setup udev --install` or follow `open-au05 setup udev`."
                .into(),
        );
    }
    fs::write("/etc/udev/rules.d/70-open-au05.rules", UDEV_RULE_CONTENT)
        .map_err(|e| format!("failed to write udev rule: {e}"))?;
    let _ = fs::create_dir_all("/etc/modules-load.d");
    fs::write("/etc/modules-load.d/open-au05.conf", MODULES_LOAD_CONTENT)
        .map_err(|e| format!("failed to write modules-load config: {e}"))?;
    let _ = Command::new("modprobe").arg("uinput").status();
    let _ = Command::new("udevadm")
        .args(["control", "--reload-rules"])
        .status();
    let _ = Command::new("udevadm").arg("trigger").status();
    Ok(())
}

pub fn systemd_unit_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/systemd/user/open-au05.service")
}

pub fn install_systemd_service(binary_override: Option<&str>) -> Result<PathBuf, String> {
    let bin = binary_override
        .map(String::from)
        .unwrap_or_else(hooks::current_binary_path);
    let unit = format!(
        "[Unit]\n\
         Description=open-au05 — Ulanzi Vibe Key AU05 Linux daemon\n\
         After=graphical-session.target pipewire.service\n\
         PartOf=graphical-session.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={bin} daemon\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n"
    );
    let path = systemd_unit_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    fs::write(&path, unit).map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    if command_exists("systemctl") {
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        let _ = Command::new("systemctl")
            .args(["--user", "enable", "--now", "open-au05.service"])
            .status();
    }
    Ok(path)
}

pub fn uninstall_systemd_service() -> Result<(), String> {
    if command_exists("systemctl") {
        let _ = Command::new("systemctl")
            .args(["--user", "disable", "--now", "open-au05.service"])
            .status();
    }
    let path = systemd_unit_path();
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("failed to remove {}: {e}", path.display()))?;
    }
    if command_exists("systemctl") {
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
    }
    Ok(())
}

pub fn systemd_service_status() -> String {
    let path = systemd_unit_path();
    if !path.exists() {
        return format!("not installed ({})", path.display());
    }
    if command_exists("systemctl") {
        if let Ok(out) = Command::new("systemctl")
            .args(["--user", "is-active", "open-au05.service"])
            .output()
        {
            let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
            return format!("{} ({})", state, path.display());
        }
    }
    format!("unit present ({})", path.display())
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}
