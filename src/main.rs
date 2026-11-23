use clap::{Parser, Subcommand};
use open_au05::config::Config;
use open_au05::control;
use open_au05::daemon::{self, DaemonOptions};
use open_au05::dictation;
use open_au05::hooks::{self, AGENTS};
use open_au05::input::create_default_emitter;
use open_au05::protocol::{
    Control, FirmwareImage, FirmwareTarget, FirmwareUpdateInfo, FirmwareUpdater, FixedFunction,
    Hotkey, LedField, LedWorkType, LightMode, VibeMessage,
};
use open_au05::session::Session;
use open_au05::system;
use open_au05::transport::open_default_transport;
use open_au05::tui;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "open-au05",
    version,
    about = "Complete open-source Linux tool for the Ulanzi Vibe Key AU05 (fff1:00dd)"
)]
struct Cli {
    /// Explicit /dev/hidrawX device path override
    #[arg(long, global = true)]
    device: Option<String>,

    /// Explicit config.toml path override
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Emit JSON output where supported
    #[arg(long, global = true)]
    json: bool,

    /// Log raw decrypted HID frames to stderr
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show dongle + mic firmware versions, serials, flash IDs, link status, and battery
    Info,
    /// Show microphone battery level, voltage, and charging state
    Battery,
    /// List onboard flash bindings for all 6 controls
    Buttons,
    /// Program an onboard flash binding (`<hotkey>`, `fixed:<name|code>`, or `factory`)
    Bind {
        /// Control name: talk, confirm, cancel, knob-press, knob-left, knob-right
        control: Control,
        /// Binding expression (e.g. `ctrl+shift+v`, `wheel-up`, `fixed:play-pause`, `factory`)
        binding: String,
    },
    /// Restore all 6 controls to factory default fixed functions
    ResetButtons,
    /// List all supported hotkey names and media fixed functions
    Keys,
    /// Read current device settings (sleep, standby, denoise, LEDs, mic)
    Settings,
    /// Modify a device setting on the AU05 microphone
    Set {
        #[command(subcommand)]
        setting: SetCommands,
    },
    /// Run a visual hardware test (LEDs)
    Test {
        #[command(subcommand)]
        target: TestCommands,
    },
    /// Stream live key, battery, and link notices from the AU05
    Monitor,
    /// One-shot voice dictation using the AU05 microphone + configured STT backend
    Dictate {
        /// Record for N seconds (omit to press Enter when done)
        #[arg(long)]
        seconds: Option<u64>,
        /// Print transcript only without injecting into the active window
        #[arg(long)]
        no_inject: bool,
    },
    /// Run the background daemon (hotplug, dictation, key replay, host actions, AI LEDs, IPC)
    Daemon,
    /// Query the running daemon status
    Status,
    /// Reload config.toml in the running daemon
    Reload,
    /// Pause daemon key forwarding and hand control back to the AU05 hardware
    Pause,
    /// Resume daemon key forwarding
    Resume,
    /// Manage AI coding agent LED hooks (Claude Code, Codex CLI, Gemini CLI, Cursor, Copilot)
    Hooks {
        #[command(subcommand)]
        action: HooksCommands,
    },
    /// Internal fast hook entrypoint invoked by AI coding agents
    Hook {
        #[arg(long)]
        agent: String,
        #[arg(long)]
        event: Option<String>,
    },
    /// Check for or flash AU05 dongle/mic firmware updates
    Firmware {
        #[command(subcommand)]
        action: FirmwareCommands,
    },
    /// Reboot the dongle, microphone, or both
    Reboot {
        /// Target: dongle, device, or both
        #[arg(default_value = "both")]
        target: String,
        /// Delay in milliseconds before reboot
        #[arg(long, default_value_t = 500)]
        delay_ms: u32,
    },
    /// Diagnose Linux permissions, HIDRAW, uinput, PipeWire AU05 mic, clipboard, STT, and daemon
    Doctor,
    /// Print or install Linux udev rules and uinput module configuration
    Setup {
        #[command(subcommand)]
        action: SetupCommands,
    },
    /// Manage the systemd user service (`open-au05.service`)
    Service {
        #[command(subcommand)]
        action: ServiceCommands,
    },
    /// Initialize, display, or locate config.toml
    Config {
        #[command(subcommand)]
        action: ConfigCommands,
    },
    /// Launch the interactive terminal dashboard
    Tui,
}

#[derive(Subcommand)]
enum SetCommands {
    /// Auto sleep timeout (`off`, `1h`, `2h`, `3h`, `4h`, or seconds)
    Sleep { value: String },
    /// Standby status (`on` / `off`)
    Standby { value: String },
    /// Standby timeout in seconds (or `10m`, `1h`, `off`)
    StandbyTime { value: String },
    /// Enable or disable the microphone (`on` / `off`)
    Mic { value: String },
    /// Hardware noise reduction (`on` / `off`, optional --low / --high)
    Denoise {
        value: String,
        #[arg(long, default_value_t = 1600)]
        low: u16,
        #[arg(long, default_value_t = 1000)]
        high: u16,
    },
    /// Vibration motor strength (`on`, `off`, or `0..=65535`)
    Vibration { value: String },
    /// Global indicator light mode (`off`, `on`, `work`)
    LightMode { mode: LightMode },
    /// All-on brightness slider (`0..=20`)
    Brightness { level: u8 },
    /// Set a single per-LED field (`0..=3`, field `type|time|breathe-level|breathe-brightness|brightness`, value)
    Led {
        led: u8,
        field: LedField,
        value: String,
    },
    /// Set hooks mode (`on` / `off`)
    HooksMode { value: String },
    /// Set audio button system mode (`0..=255`)
    AudioButtonMode { value: u8 },
}

#[derive(Subcommand)]
enum TestCommands {
    /// Cycle LED effects across confirm, cancel, and knob LEDs and restore previous state
    Leds,
}

#[derive(Subcommand)]
enum HooksCommands {
    /// Install LED status hooks into detected AI coding agents
    Install {
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove open-au05 hooks from AI coding agents
    Uninstall {
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Show hook installation status for all supported agents
    Status,
}

#[derive(Subcommand)]
enum FirmwareCommands {
    /// Query Ulanzi's update API for newer dongle & mic firmware
    Check {
        #[arg(long, default_value = "en")]
        lang: String,
    },
    /// Flash a firmware image over HID OTA (requires --yes)
    Update {
        #[arg(long)]
        target: FirmwareTarget,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum SetupCommands {
    /// Print or install `/etc/udev/rules.d/70-open-au05.rules` + `/etc/modules-load.d/open-au05.conf`
    Udev {
        /// Write files directly (requires root / sudo)
        #[arg(long)]
        install: bool,
    },
}

#[derive(Subcommand)]
enum ServiceCommands {
    /// Install and start `~/.config/systemd/user/open-au05.service`
    Install,
    /// Stop and remove the systemd user service
    Uninstall,
    /// Check systemd user service status
    Status,
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Write default config.toml (fails if already exists unless --force)
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Print effective config path
    Path,
    /// Print effective parsed config as TOML
    Show,
}

fn open_session(device: Option<String>, verbose: bool) -> Result<Session, String> {
    let transport = open_default_transport(device).map_err(|e| e.to_string())?;
    let mut session = Session::new(transport, verbose);
    session
        .authenticate(Duration::from_secs(4))
        .map_err(|e| e.to_string())?;
    Ok(session)
}

fn parse_bool(s: &str) -> Result<bool, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "on" | "true" | "yes" | "enable" | "enabled" => Ok(true),
        "0" | "off" | "false" | "no" | "disable" | "disabled" => Ok(false),
        other => Err(format!("expected on/off or true/false, got '{other}'")),
    }
}

fn parse_duration_secs(s: &str) -> Result<u32, String> {
    let t = s.trim().to_ascii_lowercase();
    if matches!(t.as_str(), "off" | "never" | "0") {
        return Ok(0);
    }
    if let Some(h) = t.strip_suffix('h') {
        return h
            .parse::<u32>()
            .map(|v| v * 3600)
            .map_err(|_| format!("invalid hours '{s}'"));
    }
    if let Some(m) = t.strip_suffix('m') {
        return m
            .parse::<u32>()
            .map(|v| v * 60)
            .map_err(|_| format!("invalid minutes '{s}'"));
    }
    if let Some(sec) = t.strip_suffix('s') {
        return sec
            .parse::<u32>()
            .map_err(|_| format!("invalid seconds '{s}'"));
    }
    t.parse::<u32>()
        .map_err(|_| format!("invalid duration '{s}' (use e.g. off, 1h, 30m, 3600)"))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    match cli.command {
        Commands::Info => {
            let mut session = open_session(cli.device, cli.verbose)?;
            let info = session.read_info().map_err(|e| e.to_string())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&info).unwrap());
            } else {
                println!(
                    "HID Device:      {}",
                    info.hid_path.as_deref().unwrap_or("?")
                );
                if let Some(ref v) = info.dongle_version {
                    println!(
                        "Dongle Firmware: {} (customer {}, build {})",
                        v.version, v.custom_code, v.build_stamp
                    );
                }
                if let Some(ref s) = info.dongle_serial {
                    println!("Dongle Serial:   {s}");
                }
                println!(
                    "Mic Linked:      {}",
                    if info.mic_linked { "yes" } else { "no" }
                );
                if let Some(ref v) = info.device_version {
                    println!(
                        "Mic Firmware:    {} (customer {}, build {})",
                        v.version, v.custom_code, v.build_stamp
                    );
                }
                if let Some(hw) = info.hardware_version {
                    println!("Hardware Rev:    {hw}");
                }
                if let Some(ref s) = info.device_serial {
                    println!("Mic Serial:      {s}");
                }
                if let Some(ref u) = info.device_uuid {
                    println!("Mic UUID:        {u}");
                }
                if let Some(ref b) = info.battery {
                    println!(
                        "Battery:         {}% ({:.2}V, charging: {}, full: {})",
                        b.percent,
                        f64::from(b.millivolts) / 1000.0,
                        b.charging,
                        b.charge_full
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Battery => {
            let mut session = open_session(cli.device, cli.verbose)?;
            let b = session.read_battery().map_err(|e| e.to_string())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&b).unwrap());
            } else {
                println!(
                    "{}% ({:.2}V){}{}",
                    b.percent,
                    f64::from(b.millivolts) / 1000.0,
                    if b.charging { " [charging]" } else { "" },
                    if b.charge_full { " [full]" } else { "" }
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Buttons => {
            let mut session = open_session(cli.device, cli.verbose)?;
            let buttons = session.read_buttons().map_err(|e| e.to_string())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&buttons).unwrap());
            } else {
                println!("{:<12} {:<24} FIXED FUNCTION", "CONTROL", "SHORTCUT");
                for b in buttons {
                    let sc = if b.shortcut_text.is_empty() {
                        "(none — uses fixed)".to_string()
                    } else {
                        b.shortcut_text
                    };
                    println!(
                        "{:<12} {:<24} {} (0x{:02X})",
                        b.control.name(),
                        sc,
                        b.fixed_function_name,
                        b.fixed_function
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Bind { control, binding } => {
            let mut session = open_session(cli.device, cli.verbose)?;
            let spec = binding.trim();
            if spec.eq_ignore_ascii_case("factory") {
                let code = control.factory_fixed_function();
                session
                    .set_button_fixed_function(control, code)
                    .map_err(|e| e.to_string())?;
                println!(
                    "Bound {control} -> factory ({})",
                    FixedFunction::name_for_code(code)
                );
            } else if let Some(fixed) = spec.strip_prefix("fixed:") {
                let code = FixedFunction::code_for_name(fixed)
                    .ok_or_else(|| format!("unknown fixed function '{fixed}'"))?;
                session
                    .set_button_fixed_function(control, code)
                    .map_err(|e| e.to_string())?;
                println!(
                    "Bound {control} -> fixed:{} (0x{code:02X})",
                    FixedFunction::name_for_code(code)
                );
            } else {
                let keys = Hotkey::parse(spec).map_err(|e| e.to_string())?;
                session
                    .set_button_shortcut(control, &keys)
                    .map_err(|e| e.to_string())?;
                println!("Bound {control} -> {}", Hotkey::render(&keys));
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::ResetButtons => {
            let mut session = open_session(cli.device, cli.verbose)?;
            session.reset_buttons().map_err(|e| e.to_string())?;
            println!("All 6 controls restored to factory functions.");
            Ok(ExitCode::SUCCESS)
        }
        Commands::Keys => {
            println!("=== Hotkey Names (HID Codes) ===");
            for (code, names) in Hotkey::catalog() {
                println!("  0x{:04X}  {}", code, names.join(", "));
            }
            println!("\n=== Fixed Functions (use with `fixed:<name>`) ===");
            for &(name, code) in FixedFunction::NAMED {
                println!("  0x{code:02X}    {name}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Settings => {
            let mut session = open_session(cli.device, cli.verbose)?;
            let s = session.read_settings().map_err(|e| e.to_string())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&s).unwrap());
            } else {
                println!("Sleep time:        {:?}s", s.sleep_time_secs);
                println!("Standby status:    {:?}", s.standby_status);
                println!("Standby time:      {:?}s", s.standby_time_secs);
                println!("Mic enabled:       {:?}", s.mic_enabled);
                println!("Noise reduction:   {:?}", s.noise_reduction);
                println!("Motor strength:    {:?}", s.motor_strength);
                println!("Audio button mode: {:?}", s.audio_button_mode);
                println!("Hooks mode:        {:?}", s.hooks_mode);
                if let Some(ref l) = s.indicator_lights {
                    let mode_name = LightMode::from_u8(l.mode)
                        .map(LightMode::name)
                        .unwrap_or("?");
                    println!(
                        "Lights:            mode={} ({}), all_on_brightness={}",
                        mode_name, l.mode, l.all_on_brightness
                    );
                    for (i, led) in l.leds.iter().enumerate() {
                        let wt = LedWorkType::from_u8(led.work_type)
                            .map(LedWorkType::name)
                            .unwrap_or("?");
                        println!(
                            "  LED {i}: type={wt}({}), time={}, breathe_lvl={}, breathe_br={}, always_br={}",
                            led.work_type,
                            led.work_time,
                            led.breathe_level,
                            led.breathe_brightness,
                            led.always_on_brightness
                        );
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Set { setting } => {
            let mut session = open_session(cli.device, cli.verbose)?;
            match setting {
                SetCommands::Sleep { value } => {
                    let secs = parse_duration_secs(&value)?;
                    session.set_sleep_time(secs).map_err(|e| e.to_string())?;
                    println!("Sleep time set to {secs}s.");
                }
                SetCommands::Standby { value } => {
                    let on = parse_bool(&value)?;
                    session.set_standby_status(on).map_err(|e| e.to_string())?;
                    println!("Standby status set to {on}.");
                }
                SetCommands::StandbyTime { value } => {
                    let secs = parse_duration_secs(&value)?;
                    session.set_standby_time(secs).map_err(|e| e.to_string())?;
                    println!("Standby time set to {secs}s.");
                }
                SetCommands::Mic { value } => {
                    let on = parse_bool(&value)?;
                    session.set_mic_enable(on).map_err(|e| e.to_string())?;
                    println!("Mic enable set to {on}.");
                }
                SetCommands::Denoise { value, low, high } => {
                    let on = parse_bool(&value)?;
                    session
                        .set_noise_reduction(u8::from(on), low, high)
                        .map_err(|e| e.to_string())?;
                    println!("Noise reduction set to {on} (low={low}, high={high}).");
                }
                SetCommands::Vibration { value } => {
                    let val = if let Ok(b) = parse_bool(&value) {
                        if b {
                            255
                        } else {
                            0
                        }
                    } else {
                        value
                            .parse::<u16>()
                            .map_err(|_| format!("invalid vibration value '{value}'"))?
                    };
                    session.set_motor_strength(val).map_err(|e| e.to_string())?;
                    println!("Motor strength set to {val}.");
                }
                SetCommands::LightMode { mode } => {
                    session.set_light_mode(mode).map_err(|e| e.to_string())?;
                    println!("Light mode set to {}.", mode.name());
                }
                SetCommands::Brightness { level } => {
                    session
                        .set_all_on_brightness(level)
                        .map_err(|e| e.to_string())?;
                    println!("All-on brightness set to {}.", level.min(20));
                }
                SetCommands::Led { led, field, value } => {
                    if led > 3 {
                        return Err("LED index must be 0..=3".into());
                    }
                    let num = if field == LedField::WorkType {
                        value
                            .parse::<LedWorkType>()
                            .map(|w| w as u8)
                            .or_else(|_| value.parse::<u8>().map_err(|e| e.to_string()))?
                    } else {
                        value
                            .parse::<u8>()
                            .map_err(|_| format!("invalid byte value '{value}'"))?
                    };
                    session
                        .set_led_field(led, field, num)
                        .map_err(|e| e.to_string())?;
                    println!("LED {led} {} set to {num}.", field.name());
                }
                SetCommands::HooksMode { value } => {
                    let on = parse_bool(&value)?;
                    session.set_hooks_mode(on).map_err(|e| e.to_string())?;
                    println!("Hooks mode set to {on}.");
                }
                SetCommands::AudioButtonMode { value } => {
                    session
                        .set_audio_button_mode(value)
                        .map_err(|e| e.to_string())?;
                    println!("Audio button mode set to {value}.");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Test { target } => {
            let mut session = open_session(cli.device, cli.verbose)?;
            match target {
                TestCommands::Leds => {
                    daemon::run_led_test_sequence(&mut session)?;
                    println!("LED test complete (original settings restored).");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Monitor => {
            let mut session = open_session(cli.device, cli.verbose)?;
            let _ = session.takeover();
            let running = Arc::new(AtomicBool::new(true));
            let r = running.clone();
            let _ = ctrlc::set_handler(move || r.store(false, Ordering::SeqCst));
            eprintln!("Monitoring AU05 events (Ctrl-C to exit)...");
            while running.load(Ordering::SeqCst) {
                match session.poll_events() {
                    Ok(events) => {
                        for msg in events {
                            match msg {
                                VibeMessage::Heartbeat => {}
                                VibeMessage::KeyEvent(ev) => println!(
                                    "KEY      control={:<10} action={} (idx={}, phys={}, status={})",
                                    ev.control().map(Control::name).unwrap_or("?"),
                                    if ev.is_press() { "PRESS" } else { "RELEASE" },
                                    ev.index,
                                    ev.physical_index,
                                    ev.status
                                ),
                                VibeMessage::BatteryNotice(b) | VibeMessage::Battery(b) => println!(
                                    "BATTERY  {}% ({:.2}V, charging={})",
                                    b.percent,
                                    f64::from(b.millivolts) / 1000.0,
                                    b.charging
                                ),
                                VibeMessage::LinkActive(on) => println!("LINK     online={on}"),
                                VibeMessage::ChargingNotice(on) => {
                                    println!("CHARGING active={on}")
                                }
                                VibeMessage::PowerOn => println!("POWER_ON mic woke up"),
                                other => println!("EVENT    {other:?}"),
                            }
                        }
                    }
                    Err(e) => return Err(e.to_string()),
                }
                thread::sleep(Duration::from_millis(12));
            }
            let _ = session.hand_back();
            Ok(ExitCode::SUCCESS)
        }
        Commands::Dictate { seconds, no_inject } => {
            let cfg = Config::load(cli.config.as_deref())?;
            let mut emitter = create_default_emitter();
            let (text, _) = dictation::run_oneshot_dictation(
                &cfg.dictation,
                seconds,
                !no_inject,
                if no_inject {
                    None
                } else {
                    Some(emitter.as_mut())
                },
            )?;
            println!("{text}");
            Ok(ExitCode::SUCCESS)
        }
        Commands::Daemon => {
            daemon::run_daemon(DaemonOptions {
                config_path: cli.config,
                device_override: cli.device,
                verbose: cli.verbose,
            })?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Status => {
            let resp = control::send_command(&json!({"cmd": "status"}))?;
            println!("{}", serde_json::to_string_pretty(&resp).unwrap());
            Ok(ExitCode::SUCCESS)
        }
        Commands::Reload => {
            let resp = control::send_command(&json!({"cmd": "reload"}))?;
            println!("{}", serde_json::to_string_pretty(&resp).unwrap());
            Ok(ExitCode::SUCCESS)
        }
        Commands::Pause => {
            let resp = control::send_command(&json!({"cmd": "pause"}))?;
            println!("{}", serde_json::to_string_pretty(&resp).unwrap());
            Ok(ExitCode::SUCCESS)
        }
        Commands::Resume => {
            let resp = control::send_command(&json!({"cmd": "resume"}))?;
            println!("{}", serde_json::to_string_pretty(&resp).unwrap());
            Ok(ExitCode::SUCCESS)
        }
        Commands::Hooks { action } => {
            let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()));
            let bin = hooks::current_binary_path();
            match action {
                HooksCommands::Install { agent, dry_run } => {
                    let targets: Vec<_> = match agent {
                        Some(id) => vec![hooks::find_agent(&id)
                            .ok_or_else(|| format!("unknown agent '{id}'"))?],
                        None => AGENTS.iter().collect(),
                    };
                    for a in targets {
                        let r = hooks::install_hooks(a, &home, &bin, None, dry_run)?;
                        if let Some(ref skip) = r.skipped {
                            println!("  [skip] {:<14} {} ({skip})", r.agent, r.path);
                        } else {
                            println!(
                                "  [ok]   {:<14} {} (added {}, updated {})",
                                r.agent, r.path, r.added, r.current
                            );
                        }
                    }
                }
                HooksCommands::Uninstall { agent, dry_run } => {
                    let targets: Vec<_> = match agent {
                        Some(id) => vec![hooks::find_agent(&id)
                            .ok_or_else(|| format!("unknown agent '{id}'"))?],
                        None => AGENTS.iter().collect(),
                    };
                    for a in targets {
                        let r = hooks::uninstall_hooks(a, &home, None, dry_run)?;
                        if let Some(ref skip) = r.skipped {
                            println!("  [skip] {:<14} {} ({skip})", r.agent, r.path);
                        } else {
                            println!(
                                "  [ok]   {:<14} {} (removed {})",
                                r.agent, r.path, r.removed
                            );
                        }
                    }
                }
                HooksCommands::Status => {
                    for a in AGENTS {
                        let (installed, total, path) = hooks::hook_status(a, &home);
                        println!("  {:<14} {installed:>2}/{total:<2}  {path}", a.id);
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Hook { agent, event } => {
            let code = hooks::run_hook_client(&agent, event.as_deref());
            Ok(ExitCode::from(code as u8))
        }
        Commands::Firmware { action } => match action {
            FirmwareCommands::Check { lang } => {
                let mut session = open_session(cli.device, cli.verbose)?;
                let info = session.read_info().map_err(|e| e.to_string())?;
                let serial = info.dongle_serial.as_deref().unwrap_or("");
                let mut results = Vec::new();
                if let Some(ref dv) = info.dongle_version {
                    results.push(FirmwareUpdateInfo::check(
                        serial,
                        FirmwareTarget::Dongle,
                        &dv.version,
                        &lang,
                    )?);
                }
                if let Some(ref mv) = info.device_version {
                    results.push(FirmwareUpdateInfo::check(
                        serial,
                        FirmwareTarget::Device,
                        &mv.version,
                        &lang,
                    )?);
                }
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&results).unwrap());
                } else {
                    for r in results {
                        println!(
                            "{:<8} current={} update_available={} latest={}",
                            r.target,
                            r.current_version,
                            r.need_update,
                            r.version.as_deref().unwrap_or("-")
                        );
                        if let Some(ref url) = r.download_url {
                            println!("  url:   {url}");
                        }
                        if let Some(ref notes) = r.notes {
                            println!("  notes: {notes}");
                        }
                    }
                }
                Ok(ExitCode::SUCCESS)
            }
            FirmwareCommands::Update { target, file, yes } => {
                let bytes = fs::read(&file)
                    .map_err(|e| format!("failed to read {}: {e}", file.display()))?;
                let image = FirmwareImage::parse(&bytes, target).map_err(|e| e.to_string())?;
                if !yes {
                    return Err(format!(
                        "parsed valid {target} image ({} bytes, {} frames, checksum 0x{:08X}). Re-run with `--yes` to flash hardware.",
                        image.payload_length(),
                        image.frames.len(),
                        image.total_checksum()
                    ));
                }
                let mut session = open_session(cli.device, cli.verbose)?;
                let updater = FirmwareUpdater::default();
                updater
                    .run(&mut session, &image, target, |frac, msg| {
                        eprintln!("[{:>3}%] {msg}", (frac * 100.0) as u32);
                    })
                    .map_err(|e| e.to_string())?;
                println!("Firmware update succeeded. Rebooting {target}...");
                match target {
                    FirmwareTarget::Dongle => {
                        let _ = session.reboot_dongle(500);
                    }
                    FirmwareTarget::Device => {
                        let _ = session.reboot_device(500);
                    }
                }
                Ok(ExitCode::SUCCESS)
            }
        },
        Commands::Reboot { target, delay_ms } => {
            let mut session = open_session(cli.device, cli.verbose)?;
            match target.to_ascii_lowercase().as_str() {
                "dongle" | "rx" => {
                    session.reboot_dongle(delay_ms).map_err(|e| e.to_string())?;
                    println!("Dongle reboot requested.");
                }
                "device" | "mic" | "tx" => {
                    session.reboot_device(delay_ms).map_err(|e| e.to_string())?;
                    println!("Mic reboot requested.");
                }
                "both" => {
                    let _ = session.reboot_device(delay_ms);
                    session.reboot_dongle(delay_ms).map_err(|e| e.to_string())?;
                    println!("Mic and dongle reboot requested.");
                }
                other => return Err(format!("invalid reboot target '{other}'")),
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Doctor => {
            let items = system::run_doctor(cli.config.as_deref());
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&items).unwrap());
            } else {
                for item in &items {
                    let icon = if item.ok { "✓" } else { "✗" };
                    println!("{icon} {:<46} {}", item.name, item.detail);
                    if let Some(ref fix) = item.fix {
                        println!("    -> {fix}");
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Setup { action } => match action {
            SetupCommands::Udev { install } => {
                if install {
                    system::install_udev_rules()?;
                    println!("Installed /etc/udev/rules.d/70-open-au05.rules and reloaded udev. Replug the AU05 USB receiver.");
                } else {
                    system::print_udev_setup_instructions();
                }
                Ok(ExitCode::SUCCESS)
            }
        },
        Commands::Service { action } => match action {
            ServiceCommands::Install => {
                let path = system::install_systemd_service(None)?;
                println!(
                    "Installed and started systemd user service: {}",
                    path.display()
                );
                Ok(ExitCode::SUCCESS)
            }
            ServiceCommands::Uninstall => {
                system::uninstall_systemd_service()?;
                println!("Uninstalled open-au05 systemd user service.");
                Ok(ExitCode::SUCCESS)
            }
            ServiceCommands::Status => {
                println!("{}", system::systemd_service_status());
                Ok(ExitCode::SUCCESS)
            }
        },
        Commands::Config { action } => match action {
            ConfigCommands::Init { force } => {
                let p = cli.config.unwrap_or_else(Config::default_path);
                if p.exists() && !force {
                    return Err(format!(
                        "{} already exists (pass --force to overwrite)",
                        p.display()
                    ));
                }
                let saved = Config::default().save(Some(&p))?;
                println!("Wrote default config to {}", saved.display());
                Ok(ExitCode::SUCCESS)
            }
            ConfigCommands::Path => {
                let p = cli.config.unwrap_or_else(Config::default_path);
                println!("{}", p.display());
                Ok(ExitCode::SUCCESS)
            }
            ConfigCommands::Show => {
                let cfg = Config::load(cli.config.as_deref())?;
                println!("{}", toml::to_string_pretty(&cfg).unwrap());
                Ok(ExitCode::SUCCESS)
            }
        },
        Commands::Tui => {
            tui::run_tui()?;
            Ok(ExitCode::SUCCESS)
        }
    }
}
