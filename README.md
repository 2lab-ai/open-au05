# open-au05

Complete open-source **Linux** driver, background daemon, voice dictation engine, onboard flash programmer, indicator LED controller, AI agent hook bridge, and interactive TUI for the **Ulanzi Vibe Key AU05** (`fff1:00dd`).

Pure Rust, single binary, zero C library dependencies (`hidapi` / `libudev` / `WebKit` not required). Talks directly to Linux `/dev/hidraw*`, `/dev/uinput`, and PipeWire/PulseAudio — built for **Wayland** (KDE Plasma 6, Hyprland, Sway, Niri, GNOME) and **X11**.

## Features

| Capability | Ulanzi Studio (macOS/Win only) | `open-au05` (Linux) |
| --- | --- | --- |
| Dongle & mic info, serials, firmware versions, battery telemetry | Settings dialog | `open-au05 info`, `open-au05 battery`, `open-au05 monitor`, TUI |
| Voice dictation on Talk key (K1) — Push-to-Talk & Toggle, Cancel key abort | macOS Fn dictation | Built-in PipeWire/PulseAudio recorder (`AU05` auto-detected) + OpenAI-compatible HTTP STT (OpenAI, Groq, local `whisper.cpp` server, `speaches`, `faster-whisper-server`) or local CLI STT + Unicode/Korean Wayland/X11 injection |
| Onboard flash button & knob programming (all 6 controls + media fixed functions) | Profile editor | `open-au05 buttons`, `open-au05 bind`, `open-au05 reset-buttons` |
| Host actions per control (`dictate`, `keys`, `run`, `open`, `type`, `noop`) | Plugin actions | `[actions]` in `~/.config/open-au05/config.toml` + native `/dev/uinput` virtual keyboard/mouse/media keys |
| Device settings (sleep timeout, standby, mic mute, noise reduction, light mode, brightness, per-LED effects) | Settings dialog | `open-au05 settings`, `open-au05 set ...`, `open-au05 test leds` |
| AI coding agent status on AU05 LEDs (Claude Code, Codex CLI, Gemini CLI, Cursor, Copilot) | `ustudio-cli` hooks | `open-au05 hooks install`, multi-session priority aggregator |
| Firmware update check & OTA flash + dongle/mic reboot | Update dialog | `open-au05 firmware check`, `open-au05 firmware update`, `open-au05 reboot` |
| Diagnostics, udev rules, systemd user service, live TUI dashboard | — | `open-au05 doctor`, `open-au05 setup udev`, `open-au05 service install`, `open-au05 tui` |

## Quick Start (Linux)

### 1. Build & install

```sh
git clone https://github.com/2lab-ai/open-au05.git
cd open-au05
cargo build --release
install -Dm755 target/release/open-au05 ~/.local/bin/open-au05
```

### 2. Grant non-root access to the AU05 vendor interface & `/dev/uinput`

```sh
open-au05 setup udev
# Or install directly with sudo:
sudo ~/.local/bin/open-au05 setup udev --install
```

Unplug and replug the AU05 USB receiver once, then verify everything with:

```sh
open-au05 doctor
open-au05 info
```

### 3. Initialize config & run the daemon

```sh
open-au05 config init
# Edit ~/.config/open-au05/config.toml (e.g., set STT API URL / key env or local whisper-cli)
open-au05 daemon
# Or run persistently as a systemd user service:
open-au05 service install
```

### 4. Interactive TUI dashboard

```sh
open-au05 tui
```

## Voice Dictation (Wayland & X11)

By default, holding the **Talk key (K1)** records 16 kHz mono WAV audio from the AU05 USB microphone (`alsa_input.usb-AU05_*` via `pw-record` / `parecord` / `arecord` / `ffmpeg`), transcribes on release, and inserts Unicode text (including Korean/CJK) into your active terminal or GUI window via clipboard-preserving paste (`wl-copy` + `uinput` `Ctrl+Shift+V` / `Ctrl+V`). Pressing **Cancel (K3)** while recording aborts cleanly.

### STT backend options in `~/.config/open-au05/config.toml`

**Option A: OpenAI / Groq / Local Whisper HTTP Server** (default):

```toml
[dictation]
enabled = true
mode = "push-to-talk"          # or "toggle"
audio_source = "auto"          # auto-finds alsa_input.usb-AU05_*
stt_backend = "openai"
api_url = "https://api.openai.com/v1/audio/transcriptions" # or http://127.0.0.1:8080/v1/audio/transcriptions
api_key_env = "OPENAI_API_KEY" # leave empty "" for local servers without auth
model = "whisper-1"            # or "whisper-large-v3-turbo" on Groq
language = "ko"                # or "" for auto-detect
inject_method = "clipboard-paste" # or "wtype", "xdotool", "clipboard-only"
paste_shortcut = "ctrl-shift-v"   # "ctrl-shift-v" for terminals, "ctrl-v" for GUI apps
restore_clipboard = true
```

**Option B: Local CLI (zero network, offline `whisper.cpp`)**:

```toml
[dictation]
stt_backend = "command"
command = "whisper-cli -m ~/.local/share/whisper/ggml-base.bin -l ko -nt -f {wav}"
```

Test dictation standalone without the daemon:

```sh
open-au05 dictate --seconds 4
```

## Onboard Controls & Host Actions

The 6 physical controls are `talk`, `confirm`, `cancel`, `knob-press`, `knob-left`, and `knob-right`.

```sh
open-au05 buttons
open-au05 bind confirm return
open-au05 bind cancel escape
open-au05 bind knob-press fixed:play-pause
open-au05 bind knob-left wheel-up
open-au05 bind knob-right wheel-down
open-au05 reset-buttons
open-au05 keys
```

While the daemon is running, controls can trigger onboard bindings (replayed cleanly via `/dev/uinput`) or host actions (`dictate`, `keys`, `run`, `open`, `type`, `noop`). Stopping or pausing the daemon (`open-au05 pause`) sends `HAND_BACK` so onboard flash bindings resume immediately.

## LEDs & AI Coding Agent Hooks

```sh
open-au05 set light-mode work
open-au05 set brightness 12
open-au05 set led 1 type breathe
open-au05 test leds

# Connect Claude Code, Codex CLI, Gemini CLI, Cursor Agent, Copilot CLI:
open-au05 hooks install
open-au05 hooks status
```

Default LED states (LED 1 = Confirm, LED 2 = Cancel, LED 3 = Knob):

| State | Confirm (LED 1) | Cancel (LED 2) | Knob (LED 3) |
| --- | --- | --- | --- |
| `recording` (dictation) | breathe | off | breathe |
| `transcribing` (dictation) | solid | breathe | breathe |
| `notification` (permission prompt) | breathe | breathe | solid |
| `attention` (waiting for input) | solid | off | solid |
| `error` | off | solid | solid |
| `working` / `thinking` / `sweeping` | off | off | breathe |
| `idle` | off | off | solid |

## Protocol & Acknowledgments

The AU05 vendor channel (`fff1:00dd`, interface 3, usage page `0xFFFC`, report ID `0x55`, 32-round TEA encryption, challenge-response auth, heartbeat, settings, LEDs, button shortcuts, and OTA protocol) builds on community reverse-engineering work documented in [vibed](https://github.com/ihavespoons/vibed), [clanzi](https://github.com/JXNCTED/clanzi), [Olanzi](https://github.com/MrCroxx/olanzi), and [Typist](https://github.com/OmiroAI/ulanzi-vibekey).

## License

MIT — see [LICENSE](LICENSE). `open-au05` is an independent open-source project and is not affiliated with Ulanzi.
