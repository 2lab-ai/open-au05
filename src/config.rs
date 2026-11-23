//! Configuration schema and XDG path resolution for `open-au05`.

use crate::protocol::{Control, LedWorkType};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub device: DeviceConfig,
    pub buttons: BTreeMap<String, String>,
    pub actions: BTreeMap<String, HostAction>,
    pub dictation: DictationConfig,
    pub agent_lights: AgentLightsConfig,
}

impl Default for Config {
    fn default() -> Self {
        let mut buttons = BTreeMap::new();
        buttons.insert("confirm".into(), "return".into());
        buttons.insert("cancel".into(), "escape".into());
        buttons.insert("knob-press".into(), "backspace".into());
        buttons.insert("knob-left".into(), "wheel-up".into());
        buttons.insert("knob-right".into(), "wheel-down".into());

        let mut actions = BTreeMap::new();
        actions.insert(
            "talk".into(),
            HostAction {
                dictate: true,
                keys: None,
                run: None,
                open: None,
                r#type: None,
                noop: false,
                on: TriggerEdge::Both,
            },
        );

        Self {
            device: DeviceConfig::default(),
            buttons,
            actions,
            dictation: DictationConfig::default(),
            agent_lights: AgentLightsConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceConfig {
    /// Auto sleep timeout in seconds (`0` = never; Ulanzi presets: 3600, 7200, 10800, 14400).
    pub sleep_time: Option<u32>,
    pub standby_time: Option<u32>,
    pub denoise: Option<bool>,
    /// 0 = off, 1 = all-on, 2 = per-LED work mode.
    pub light_mode: Option<u8>,
    /// All-on brightness (0..=20).
    pub brightness: Option<u8>,
    /// Work-mode LED duration (255 = stay on while active).
    pub led_work_time: u8,
    pub hooks_mode: Option<bool>,
    pub audio_button_mode: Option<u8>,
    /// Sync changed settings & onboard button bindings to flash whenever the mic connects.
    pub sync_on_connect: bool,
    /// Replay onboard firmware button bindings via `/dev/uinput` when no host action overrides them.
    pub replay_bindings: bool,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            sleep_time: Some(3600),
            standby_time: None,
            denoise: Some(true),
            light_mode: Some(2),
            brightness: Some(10),
            led_work_time: 255,
            hooks_mode: None,
            audio_button_mode: None,
            sync_on_connect: true,
            replay_bindings: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TriggerEdge {
    #[default]
    Press,
    Release,
    Both,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HostAction {
    /// Route this control to the built-in voice dictation engine.
    pub dictate: bool,
    /// Synthetic hotkey combo emitted via `/dev/uinput` (e.g. `"ctrl+shift+v"`).
    pub keys: Option<String>,
    /// Shell command to run via `sh -c`.
    pub run: Option<String>,
    /// URL or file path to open via `xdg-open`.
    pub open: Option<String>,
    /// Literal Unicode text to inject into the active window.
    pub r#type: Option<String>,
    /// Explicitly swallow this control without replaying its firmware binding.
    pub noop: bool,
    /// Which edge triggers the action (`press`, `release`, `both`).
    pub on: TriggerEdge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DictationMode {
    PushToTalk,
    Toggle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecorderBackend {
    Auto,
    PwRecord,
    Parecord,
    Arecord,
    Ffmpeg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SttBackend {
    Openai,
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InjectMethod {
    ClipboardPaste,
    Wtype,
    Xdotool,
    ClipboardOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PasteShortcut {
    CtrlShiftV,
    CtrlV,
    ShiftInsert,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DictationConfig {
    pub enabled: bool,
    pub mode: DictationMode,
    /// Pressing the Cancel key (K3) aborts an in-progress recording.
    pub cancel_with_cancel_key: bool,
    /// `"auto"` discovers `alsa_input.usb-AU05_*` via PipeWire/PulseAudio, or specify an explicit source name.
    pub audio_source: String,
    pub recorder: RecorderBackend,
    pub stt_backend: SttBackend,
    /// OpenAI-compatible transcription endpoint (works with OpenAI, Groq, local whisper.cpp server, speaches, faster-whisper-server).
    pub api_url: String,
    /// Environment variable containing the Bearer API key (leave unset or empty for local keyless servers).
    pub api_key_env: String,
    pub model: String,
    /// Optional BCP-47 language hint (e.g. `"ko"`, `"en"`, or empty for auto-detect).
    pub language: Option<String>,
    /// Optional Whisper prompt / vocabulary hint.
    pub prompt: Option<String>,
    /// Local CLI STT command template when `stt_backend = "command"`. Use `{wav}` for the recorded file path.
    pub command: Option<String>,
    pub inject_method: InjectMethod,
    pub paste_shortcut: PasteShortcut,
    pub restore_clipboard: bool,
    pub auto_enter: bool,
    /// Ignore accidental taps shorter than this many milliseconds in push-to-talk mode.
    pub min_duration_ms: u64,
    /// Send a desktop notification (`notify-send`) on errors or completions.
    pub notify: bool,
}

impl Default for DictationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: DictationMode::PushToTalk,
            cancel_with_cancel_key: true,
            audio_source: "auto".into(),
            recorder: RecorderBackend::Auto,
            stt_backend: SttBackend::Openai,
            api_url: "https://api.openai.com/v1/audio/transcriptions".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            model: "whisper-1".into(),
            language: Some("ko".into()),
            prompt: None,
            command: Some(
                "whisper-cli -m ~/.local/share/whisper/ggml-base.bin -l ko -nt -f {wav}".into(),
            ),
            inject_method: InjectMethod::ClipboardPaste,
            paste_shortcut: PasteShortcut::CtrlShiftV,
            restore_clipboard: true,
            auto_enter: false,
            min_duration_ms: 250,
            notify: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentLightsConfig {
    pub enabled: bool,
    /// `"open-au05"` (distinct states) or `"ulanzi"` (matches Ulanzi Studio 3.3.9).
    pub preset: String,
    /// Optional custom overrides: state name -> map of LED index ("1"|"2"|"3") -> work_type (0|1|2).
    pub states: BTreeMap<String, BTreeMap<String, u8>>,
}

impl Default for AgentLightsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            preset: "open-au05".into(),
            states: BTreeMap::new(),
        }
    }
}

impl AgentLightsConfig {
    /// Returns `[led1_confirm, led2_cancel, led3_knob]` work types for a given state.
    pub fn pattern_for(&self, state: &str) -> [LedWorkType; 3] {
        let mut pat = if self.preset.eq_ignore_ascii_case("ulanzi") {
            [LedWorkType::Off, LedWorkType::Off, LedWorkType::Solid]
        } else {
            match state {
                "recording" => [LedWorkType::Breathe, LedWorkType::Off, LedWorkType::Breathe],
                "transcribing" => [
                    LedWorkType::Solid,
                    LedWorkType::Breathe,
                    LedWorkType::Breathe,
                ],
                "notification" => [
                    LedWorkType::Breathe,
                    LedWorkType::Breathe,
                    LedWorkType::Solid,
                ],
                "error" => [LedWorkType::Off, LedWorkType::Solid, LedWorkType::Solid],
                "attention" => [LedWorkType::Solid, LedWorkType::Off, LedWorkType::Solid],
                "working" | "thinking" | "sweeping" => {
                    [LedWorkType::Off, LedWorkType::Off, LedWorkType::Breathe]
                }
                "sleeping" => [LedWorkType::Off, LedWorkType::Off, LedWorkType::Off],
                _ => [LedWorkType::Off, LedWorkType::Off, LedWorkType::Solid], // idle
            }
        };
        if let Some(custom) = self.states.get(state) {
            for (k, &v) in custom {
                if let Some(wt) = LedWorkType::from_u8(v) {
                    match k.as_str() {
                        "1" | "confirm" => pat[0] = wt,
                        "2" | "cancel" => pat[1] = wt,
                        "3" | "knob" => pat[2] = wt,
                        _ => {}
                    }
                }
            }
        }
        pat
    }
}

impl Config {
    pub fn default_path() -> PathBuf {
        if let Ok(explicit) = std::env::var("OPEN_AU05_CONFIG") {
            return PathBuf::from(explicit);
        }
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("open-au05").join("config.toml");
            }
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home)
            .join(".config")
            .join("open-au05")
            .join("config.toml")
    }

    pub fn load(path: Option<&Path>) -> Result<Self, String> {
        let p = path.map(PathBuf::from).unwrap_or_else(Self::default_path);
        if !p.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&p)
            .map_err(|e| format!("failed to read config {}: {e}", p.display()))?;
        toml::from_str(&text).map_err(|e| format!("invalid TOML in {}: {e}", p.display()))
    }

    pub fn save(&self, path: Option<&Path>) -> Result<PathBuf, String> {
        let p = path.map(PathBuf::from).unwrap_or_else(Self::default_path);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
        }
        let text =
            toml::to_string_pretty(self).map_err(|e| format!("failed to serialize config: {e}"))?;
        fs::write(&p, text).map_err(|e| format!("failed to write {}: {e}", p.display()))?;
        Ok(p)
    }

    pub fn action_for(&self, control: Control) -> Option<&HostAction> {
        self.actions.get(control.name())
    }
}
