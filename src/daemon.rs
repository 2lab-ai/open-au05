//! Background daemon managing the AU05 USB receiver, hotplug reconnects,
//! config-to-flash syncing, voice dictation, host actions, uinput replay,
//! AI agent LEDs, and the Unix control socket.

use crate::config::{Config, HostAction, TriggerEdge};
use crate::control::ControlServer;
use crate::dictation::{DictationController, DictationState};
use crate::hooks::{AgentTracker, TrackedSession};
use crate::input::inject::{inject_text, TextInjectorConfig};
use crate::input::scheduler::KeyScheduler;
use crate::input::{create_default_emitter, InputEmitter};
use crate::protocol::{
    BatteryStatus, Control, FixedFunction, Hotkey, IndicatorLights, KeyEvent, LedField,
    LedWorkType, LightMode, VibeMessage,
};
use crate::session::{ButtonBindingInfo, Session};
use crate::transport::open_default_transport;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub connected: bool,
    pub authenticated: bool,
    pub mic_linked: bool,
    pub paused: bool,
    pub device_path: Option<String>,
    pub battery: Option<BatteryStatus>,
    pub dictation_state: DictationState,
    pub last_transcript: Option<String>,
    pub last_error: Option<String>,
    pub active_led_state: String,
    pub led_pattern: [String; 3],
    pub agent_sessions: Vec<TrackedSession>,
    pub recent_events: Vec<String>,
    pub buttons: Vec<ButtonBindingInfo>,
}

pub struct DaemonOptions {
    pub config_path: Option<PathBuf>,
    pub device_override: Option<String>,
    pub verbose: bool,
}

pub fn run_daemon(opts: DaemonOptions) -> Result<(), String> {
    let mut cfg = Config::load(opts.config_path.as_deref())?;
    let server = ControlServer::bind()?;
    eprintln!(
        "[open-au05] daemon started (socket: {}, config: {})",
        server.path().display(),
        opts.config_path
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| Config::default_path().display().to_string())
    );

    let running = Arc::new(AtomicBool::new(true));
    let running_flag = running.clone();
    let _ = ctrlc::set_handler(move || {
        running_flag.store(false, Ordering::SeqCst);
    });

    let mut session: Option<Session> = None;
    let mut emitter: Box<dyn InputEmitter> = create_default_emitter();
    let _ = emitter.ensure();
    let mut scheduler = KeyScheduler::default();
    let mut dictation = DictationController::default();
    let mut agents = AgentTracker::default();

    let mut paused = false;
    let mut mic_linked = false;
    let mut battery: Option<BatteryStatus> = None;
    let mut cached_buttons: BTreeMap<Control, ButtonBindingInfo> = BTreeMap::new();
    let mut last_led_pattern: Option<[LedWorkType; 3]> = None;
    let mut active_led_state = "idle".to_string();
    let mut last_error: Option<String> = None;
    let mut recent_events: VecDeque<String> = VecDeque::with_capacity(64);

    let mut next_connect_attempt = Instant::now();
    let mut next_battery_poll = Instant::now();

    while running.load(Ordering::SeqCst) {
        // 1. Hotplug connect / reconnect
        if session.is_none() && Instant::now() >= next_connect_attempt {
            match open_default_transport(opts.device_override.clone()) {
                Ok(transport) => {
                    let mut s = Session::new(transport, opts.verbose);
                    match s.authenticate(Duration::from_secs(4)) {
                        Ok(()) => {
                            log_event(
                                &mut recent_events,
                                format!(
                                    "connected & authenticated ({})",
                                    s.device_path().unwrap_or("unknown")
                                ),
                            );
                            last_error = None;
                            mic_linked = s.is_mic_linked().unwrap_or(false);
                            if !paused {
                                let _ = s.takeover();
                            }
                            if mic_linked {
                                on_mic_linked(
                                    &mut s,
                                    &cfg,
                                    &mut cached_buttons,
                                    &mut battery,
                                    &mut last_led_pattern,
                                    &mut recent_events,
                                );
                            }
                            session = Some(s);
                            next_battery_poll = Instant::now() + Duration::from_secs(20);
                        }
                        Err(e) => {
                            last_error = Some(e.to_string());
                            log_event(&mut recent_events, format!("auth failed: {e}"));
                            next_connect_attempt = Instant::now() + Duration::from_secs(2);
                        }
                    }
                }
                Err(e) => {
                    let msg = e.to_string();
                    if last_error.as_deref() != Some(msg.as_str()) {
                        log_event(&mut recent_events, format!("waiting for AU05: {msg}"));
                        last_error = Some(msg);
                    }
                    next_connect_attempt = Instant::now() + Duration::from_secs(2);
                }
            }
        }

        // 2. Poll HID events & heartbeat
        let mut disconnect_reason: Option<String> = None;
        if let Some(ref mut s) = session {
            match s.poll_events() {
                Ok(msgs) => {
                    for msg in msgs {
                        match msg {
                            VibeMessage::KeyEvent(ev) => {
                                if !paused {
                                    handle_key_event(
                                        ev,
                                        &cfg,
                                        &cached_buttons,
                                        &mut dictation,
                                        &mut scheduler,
                                        emitter.as_mut(),
                                        &mut recent_events,
                                    );
                                }
                            }
                            VibeMessage::LinkActive(linked) => {
                                mic_linked = linked;
                                log_event(
                                    &mut recent_events,
                                    format!(
                                        "mic link: {}",
                                        if linked { "online" } else { "offline" }
                                    ),
                                );
                                if linked {
                                    on_mic_linked(
                                        s,
                                        &cfg,
                                        &mut cached_buttons,
                                        &mut battery,
                                        &mut last_led_pattern,
                                        &mut recent_events,
                                    );
                                } else {
                                    let _ = scheduler.release_all(emitter.as_mut());
                                }
                            }
                            VibeMessage::PowerOn => {
                                mic_linked = true;
                                log_event(&mut recent_events, "mic powered on".into());
                                on_mic_linked(
                                    s,
                                    &cfg,
                                    &mut cached_buttons,
                                    &mut battery,
                                    &mut last_led_pattern,
                                    &mut recent_events,
                                );
                            }
                            VibeMessage::BatteryNotice(b) | VibeMessage::Battery(b) => {
                                battery = Some(b);
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    disconnect_reason = Some(e.to_string());
                }
            }

            // Periodic battery refresh while online
            if disconnect_reason.is_none() && mic_linked && Instant::now() >= next_battery_poll {
                next_battery_poll = Instant::now() + Duration::from_secs(30);
                if let Ok(b) = s.read_battery() {
                    battery = Some(b);
                }
            }
        }

        if let Some(reason) = disconnect_reason {
            log_event(&mut recent_events, format!("disconnected: {reason}"));
            let _ = scheduler.release_all(emitter.as_mut());
            session = None;
            mic_linked = false;
            last_led_pattern = None;
            next_connect_attempt = Instant::now() + Duration::from_secs(2);
        }

        // 3. Poll completed background voice transcription
        if let Some(res) = dictation.poll_completed(&cfg.dictation, Some(emitter.as_mut())) {
            match res {
                Ok(text) if !text.is_empty() => {
                    log_event(&mut recent_events, format!("dictated: {text}"));
                }
                Ok(_) => log_event(&mut recent_events, "dictation returned empty text".into()),
                Err(e) => {
                    last_error = Some(e.clone());
                    log_event(&mut recent_events, format!("dictation error: {e}"));
                }
            }
        }

        // 4. Reconcile LEDs (Dictation state > AI agent state > idle)
        if let Some(ref mut s) = session {
            if mic_linked && cfg.agent_lights.enabled {
                let (state_name, desired) = compute_led_pattern(&cfg, dictation.state, &mut agents);
                active_led_state = state_name.to_string();
                if last_led_pattern != Some(desired)
                    && apply_led_pattern(s, desired, cfg.device.led_work_time).is_ok()
                {
                    last_led_pattern = Some(desired);
                }
            }
        }

        // 5. Serve Unix control socket requests
        server.poll(|req| {
            let cmd = req.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
            match cmd {
                "status" => {
                    let pat = last_led_pattern.unwrap_or([
                        LedWorkType::Off,
                        LedWorkType::Off,
                        LedWorkType::Solid,
                    ]);
                    let st = DaemonStatus {
                        connected: session.is_some(),
                        authenticated: session.as_ref().is_some_and(Session::is_authenticated),
                        mic_linked,
                        paused,
                        device_path: session
                            .as_ref()
                            .and_then(|s| s.device_path().map(String::from)),
                        battery: battery.clone(),
                        dictation_state: dictation.state,
                        last_transcript: dictation.last_transcript.clone(),
                        last_error: last_error.clone(),
                        active_led_state: active_led_state.clone(),
                        led_pattern: [
                            pat[0].name().into(),
                            pat[1].name().into(),
                            pat[2].name().into(),
                        ],
                        agent_sessions: agents.snapshot(),
                        recent_events: recent_events.iter().cloned().collect(),
                        buttons: cached_buttons.values().cloned().collect(),
                    };
                    serde_json::to_value(st).unwrap_or_else(|_| json!({"ok": false}))
                }
                "reload" => match Config::load(opts.config_path.as_deref()) {
                    Ok(new_cfg) => {
                        cfg = new_cfg;
                        last_led_pattern = None;
                        if let Some(ref mut s) = session {
                            if mic_linked {
                                on_mic_linked(
                                    s,
                                    &cfg,
                                    &mut cached_buttons,
                                    &mut battery,
                                    &mut last_led_pattern,
                                    &mut recent_events,
                                );
                            }
                        }
                        log_event(&mut recent_events, "config reloaded".into());
                        json!({"ok": true})
                    }
                    Err(e) => json!({"ok": false, "error": e}),
                },
                "pause" => {
                    paused = true;
                    let _ = scheduler.release_all(emitter.as_mut());
                    if let Some(ref mut s) = session {
                        let _ = s.hand_back();
                    }
                    log_event(
                        &mut recent_events,
                        "paused forwarding (handed back to device)".into(),
                    );
                    json!({"ok": true, "paused": true})
                }
                "resume" => {
                    paused = false;
                    if let Some(ref mut s) = session {
                        let _ = s.takeover();
                    }
                    log_event(&mut recent_events, "resumed forwarding".into());
                    json!({"ok": true, "paused": false})
                }
                "agent-state" => {
                    let agent = req
                        .get("agent")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let state = req.get("state").and_then(|v| v.as_str()).unwrap_or("idle");
                    let sess = req
                        .get("session")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default");
                    agents.update(agent, sess, state);
                    json!({"ok": true})
                }
                "dictate-toggle" => match dictation.toggle(&cfg.dictation) {
                    Ok(msg) => {
                        if let Some(m) = msg {
                            log_event(&mut recent_events, m);
                        }
                        json!({"ok": true, "state": dictation.state})
                    }
                    Err(e) => json!({"ok": false, "error": e}),
                },
                "test-leds" => {
                    if let Some(ref mut s) = session {
                        if mic_linked {
                            let _ = run_led_test_sequence(s);
                            last_led_pattern = None;
                            return json!({"ok": true});
                        }
                    }
                    json!({"ok": false, "error": "mic not linked"})
                }
                "cycle-light-mode" => {
                    if let Some(ref mut s) = session {
                        if mic_linked {
                            if let Ok(lights) = s.read_indicator_lights() {
                                let next = match lights.mode {
                                    0 => LightMode::On,
                                    1 => LightMode::Work,
                                    _ => LightMode::Off,
                                };
                                let _ = s.set_light_mode(next);
                                last_led_pattern = None;
                                log_event(
                                    &mut recent_events,
                                    format!("light mode -> {}", next.name()),
                                );
                                return json!({"ok": true, "mode": next.name()});
                            }
                        }
                    }
                    json!({"ok": false, "error": "mic not linked"})
                }
                _ => json!({"ok": false, "error": format!("unknown command '{cmd}'")}),
            }
        });

        thread::sleep(Duration::from_millis(12));
    }

    // Graceful cleanup: release all virtual keys & return onboard control to AU05.
    let _ = dictation.cancel();
    let _ = scheduler.release_all(emitter.as_mut());
    emitter.close();
    if let Some(ref mut s) = session {
        let _ = s.hand_back();
    }
    eprintln!("[open-au05] daemon stopped cleanly");
    Ok(())
}

fn log_event(recent: &mut VecDeque<String>, msg: String) {
    eprintln!("[open-au05] {msg}");
    if recent.len() >= 50 {
        recent.pop_front();
    }
    recent.push_back(msg);
}

fn on_mic_linked(
    session: &mut Session,
    cfg: &Config,
    cached_buttons: &mut BTreeMap<Control, ButtonBindingInfo>,
    battery: &mut Option<BatteryStatus>,
    last_led_pattern: &mut Option<[LedWorkType; 3]>,
    recent: &mut VecDeque<String>,
) {
    *last_led_pattern = None;
    if let Ok(b) = session.read_battery() {
        *battery = Some(b);
    }
    if cfg.device.sync_on_connect {
        sync_config_to_device(session, cfg, recent);
    }
    if let Ok(buttons) = session.read_buttons() {
        cached_buttons.clear();
        for b in buttons {
            cached_buttons.insert(b.control, b);
        }
    }
}

fn sync_config_to_device(session: &mut Session, cfg: &Config, recent: &mut VecDeque<String>) {
    // Diff-only settings sync to avoid unnecessary flash writes.
    if let Ok(current) = session.read_settings() {
        if let Some(want) = cfg.device.sleep_time {
            if current.sleep_time_secs != Some(want) {
                let _ = session.set_sleep_time(want);
                log_event(recent, format!("synced sleep_time={want}s"));
            }
        }
        if let Some(want) = cfg.device.standby_time {
            if current.standby_time_secs != Some(want) {
                let _ = session.set_standby_time(want);
                log_event(recent, format!("synced standby_time={want}s"));
            }
        }
        if let Some(want_denoise) = cfg.device.denoise {
            let want_lvl = u8::from(want_denoise);
            if current.noise_reduction.map(|(l, _, _)| l) != Some(want_lvl) {
                let _ = session.set_noise_reduction(want_lvl, 1600, 1000);
                log_event(recent, format!("synced denoise={want_denoise}"));
            }
        }
        if let Some(want_mode) = cfg.device.light_mode {
            if current.indicator_lights.as_ref().map(|l| l.mode) != Some(want_mode) {
                if let Some(m) = LightMode::from_u8(want_mode) {
                    let _ = session.set_light_mode(m);
                    log_event(recent, format!("synced light_mode={}", m.name()));
                }
            }
        }
        if let Some(want_br) = cfg.device.brightness {
            if current
                .indicator_lights
                .as_ref()
                .map(|l| l.all_on_brightness)
                != Some(want_br)
            {
                let _ = session.set_all_on_brightness(want_br);
                log_event(recent, format!("synced brightness={want_br}"));
            }
        }
        if let Some(want_hooks) = cfg.device.hooks_mode {
            if current.hooks_mode != Some(u8::from(want_hooks)) {
                let _ = session.set_hooks_mode(want_hooks);
            }
        }
        if let Some(want_abm) = cfg.device.audio_button_mode {
            if current.audio_button_mode != Some(want_abm) {
                let _ = session.set_audio_button_mode(want_abm);
            }
        }
    }

    // Diff-only onboard button bindings sync
    if !cfg.buttons.is_empty() {
        if let Ok(existing) = session.read_buttons() {
            for b in existing {
                let Some(spec) = cfg.buttons.get(b.control.name()) else {
                    continue;
                };
                let spec_t = spec.trim();
                if spec_t.eq_ignore_ascii_case("factory") {
                    let factory = b.control.factory_fixed_function();
                    if !b.shortcut.is_empty() || b.fixed_function != factory {
                        let _ = session.set_button_fixed_function(b.control, factory);
                        log_event(recent, format!("synced {} -> factory", b.control));
                    }
                } else if let Some(fixed_spec) = spec_t.strip_prefix("fixed:") {
                    if let Some(code) = FixedFunction::code_for_name(fixed_spec) {
                        if !b.shortcut.is_empty() || b.fixed_function != code {
                            let _ = session.set_button_fixed_function(b.control, code);
                            log_event(
                                recent,
                                format!("synced {} -> fixed:{fixed_spec}", b.control),
                            );
                        }
                    }
                } else if let Ok(want_keys) = Hotkey::parse(spec_t) {
                    if b.shortcut != want_keys {
                        let _ = session.set_button_shortcut(b.control, &want_keys);
                        log_event(recent, format!("synced {} -> {spec_t}", b.control));
                    }
                }
            }
        }
    }
}

fn handle_key_event(
    ev: KeyEvent,
    cfg: &Config,
    cached_buttons: &BTreeMap<Control, ButtonBindingInfo>,
    dictation: &mut DictationController,
    scheduler: &mut KeyScheduler,
    emitter: &mut dyn InputEmitter,
    recent: &mut VecDeque<String>,
) {
    let Some(control) = ev.control() else {
        return;
    };
    let pressed = ev.is_press();

    // Cancel key aborts an active recording when configured.
    if control == Control::Cancel
        && pressed
        && cfg.dictation.cancel_with_cancel_key
        && dictation.is_recording()
        && dictation.cancel()
    {
        log_event(recent, "dictation cancelled via cancel key".into());
        return;
    }

    // Check explicit host action for this control
    if let Some(action) = cfg.action_for(control) {
        if action.dictate {
            match dictation.handle_talk_edge(pressed, &cfg.dictation) {
                Ok(Some(m)) => log_event(recent, m),
                Ok(None) => {}
                Err(e) => log_event(recent, format!("dictation error: {e}")),
            }
            return;
        }
        if action.noop {
            return;
        }
        run_host_action(control, pressed, action, scheduler, emitter, recent);
        return;
    }

    // Fallback: replay onboard firmware binding via uinput
    if !cfg.device.replay_bindings {
        return;
    }
    let Some(binding) = cached_buttons.get(&control) else {
        return;
    };
    if !binding.shortcut.is_empty() {
        let codes: Vec<u16> = binding.shortcut.iter().map(|k| k.hid_code()).collect();
        if control.is_rotation() {
            if pressed {
                let _ = emitter.ensure();
                let _ = scheduler.tap_combo(&codes, emitter);
            }
        } else if pressed {
            let _ = emitter.ensure();
            let _ = scheduler.press_control(control.index(), &codes, emitter);
        } else {
            let _ = scheduler.release_control(control.index(), emitter);
        }
    } else {
        let _ = emitter.ensure();
        let _ = emitter.emit_fixed_function(binding.fixed_function, pressed);
    }
}

fn run_host_action(
    control: Control,
    pressed: bool,
    action: &HostAction,
    scheduler: &mut KeyScheduler,
    emitter: &mut dyn InputEmitter,
    recent: &mut VecDeque<String>,
) {
    // Keys action supports natural press/release hold when on == Both or Press on non-rotation.
    if let Some(ref hotkey) = action.keys {
        if let Ok(codes) = Hotkey::parse_hid_codes(hotkey) {
            let _ = emitter.ensure();
            if control.is_rotation() {
                if pressed {
                    let _ = scheduler.tap_combo(&codes, emitter);
                }
            } else {
                match action.on {
                    TriggerEdge::Both | TriggerEdge::Press => {
                        if pressed {
                            let _ = scheduler.press_control(control.index(), &codes, emitter);
                        } else {
                            let _ = scheduler.release_control(control.index(), emitter);
                        }
                    }
                    TriggerEdge::Release => {
                        if !pressed {
                            let _ = scheduler.tap_combo(&codes, emitter);
                        }
                    }
                }
            }
        }
    }

    let edge_matches = match action.on {
        TriggerEdge::Press => pressed,
        TriggerEdge::Release => !pressed,
        TriggerEdge::Both => true,
    };
    if !edge_matches {
        return;
    }

    if let Some(ref cmd) = action.run {
        let cmd_owned = cmd.clone();
        log_event(recent, format!("action {control}: run `{cmd_owned}`"));
        thread::spawn(move || {
            let _ = Command::new("sh").args(["-c", &cmd_owned]).status();
        });
    }
    if let Some(ref target) = action.open {
        let target_owned = target.clone();
        log_event(recent, format!("action {control}: open `{target_owned}`"));
        thread::spawn(move || {
            let _ = Command::new("xdg-open").arg(&target_owned).status();
        });
    }
    if let Some(ref text) = action.r#type {
        let cfg = TextInjectorConfig::default();
        if let Err(e) = inject_text(text, &cfg, Some(emitter)) {
            log_event(recent, format!("action {control}: type failed: {e}"));
        }
    }
}

fn compute_led_pattern(
    cfg: &Config,
    dictation: DictationState,
    agents: &mut AgentTracker,
) -> (&'static str, [LedWorkType; 3]) {
    match dictation {
        DictationState::Recording => ("recording", cfg.agent_lights.pattern_for("recording")),
        DictationState::Transcribing => {
            ("transcribing", cfg.agent_lights.pattern_for("transcribing"))
        }
        DictationState::Error => ("error", cfg.agent_lights.pattern_for("error")),
        DictationState::Idle => {
            let (state, any_busy) = agents.aggregate();
            let mut pat = cfg.agent_lights.pattern_for(state);
            if any_busy && !cfg.agent_lights.preset.eq_ignore_ascii_case("ulanzi") {
                pat[2] = LedWorkType::Breathe;
            }
            (state, pat)
        }
    }
}

pub fn apply_led_pattern(
    session: &mut Session,
    pattern: [LedWorkType; 3],
    work_time: u8,
) -> Result<(), String> {
    for (idx, wt) in [(1u8, pattern[0]), (2u8, pattern[1]), (3u8, pattern[2])] {
        session
            .set_led_field(idx, LedField::WorkTime, work_time)
            .map_err(|e| e.to_string())?;
        session
            .set_led_field(idx, LedField::WorkType, wt as u8)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Visual LED test sequence cycling solid & breathing across LEDs 1 (confirm), 2 (cancel), 3 (knob).
pub fn run_led_test_sequence(session: &mut Session) -> Result<IndicatorLights, String> {
    let original = session.read_indicator_lights().map_err(|e| e.to_string())?;
    session
        .set_light_mode(LightMode::Work)
        .map_err(|e| e.to_string())?;

    let steps: &[(&str, [LedWorkType; 3])] = &[
        (
            "all off",
            [LedWorkType::Off, LedWorkType::Off, LedWorkType::Off],
        ),
        (
            "confirm solid",
            [LedWorkType::Solid, LedWorkType::Off, LedWorkType::Off],
        ),
        (
            "cancel solid",
            [LedWorkType::Off, LedWorkType::Solid, LedWorkType::Off],
        ),
        (
            "knob solid",
            [LedWorkType::Off, LedWorkType::Off, LedWorkType::Solid],
        ),
        (
            "all breathing",
            [
                LedWorkType::Breathe,
                LedWorkType::Breathe,
                LedWorkType::Breathe,
            ],
        ),
    ];
    for (label, pat) in steps {
        eprintln!("LED test: {label}");
        apply_led_pattern(session, *pat, 255)?;
        thread::sleep(Duration::from_millis(900));
    }

    // Restore original LEDs
    if let Some(mode) = LightMode::from_u8(original.mode) {
        let _ = session.set_light_mode(mode);
    }
    for (idx, led) in original.leds.iter().enumerate().skip(1) {
        let _ = session.set_led_field(idx as u8, LedField::WorkTime, led.work_time);
        let _ = session.set_led_field(idx as u8, LedField::WorkType, led.work_type);
    }
    Ok(original)
}
