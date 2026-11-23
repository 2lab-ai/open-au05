//! Voice dictation engine combining AU05 audio capture, STT transcription,
//! and Wayland/X11 text injection.

pub mod audio;
pub mod stt;

use crate::config::{DictationConfig, DictationMode};
use crate::dictation::audio::{cleanup_wav, RecordingHandle};
use crate::input::inject::{command_exists, inject_text, TextInjectorConfig};
use crate::input::InputEmitter;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DictationState {
    Idle,
    Recording,
    Transcribing,
    Error,
}

#[derive(Clone, Debug)]
pub struct DictationResult {
    pub text: Result<String, String>,
    pub duration: Duration,
}

pub struct DictationController {
    recording: Option<RecordingHandle>,
    pub state: DictationState,
    pub last_transcript: Option<String>,
    pub last_error: Option<String>,
    result_tx: Sender<DictationResult>,
    result_rx: Receiver<DictationResult>,
}

impl Default for DictationController {
    fn default() -> Self {
        let (result_tx, result_rx) = mpsc::channel();
        Self {
            recording: None,
            state: DictationState::Idle,
            last_transcript: None,
            last_error: None,
            result_tx,
            result_rx,
        }
    }
}

impl DictationController {
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    pub fn handle_talk_edge(
        &mut self,
        pressed: bool,
        cfg: &DictationConfig,
    ) -> Result<Option<String>, String> {
        if !cfg.enabled {
            return Ok(None);
        }
        match cfg.mode {
            DictationMode::PushToTalk => {
                if pressed {
                    self.start_recording(cfg)?;
                    Ok(Some("recording started (push-to-talk)".into()))
                } else if self.recording.is_some() {
                    self.finish_recording(cfg)
                } else {
                    Ok(None)
                }
            }
            DictationMode::Toggle => {
                if !pressed {
                    return Ok(None);
                }
                if self.recording.is_some() {
                    self.finish_recording(cfg)
                } else {
                    self.start_recording(cfg)?;
                    Ok(Some("recording started (toggle)".into()))
                }
            }
        }
    }

    pub fn toggle(&mut self, cfg: &DictationConfig) -> Result<Option<String>, String> {
        if self.recording.is_some() {
            self.finish_recording(cfg)
        } else {
            self.start_recording(cfg)?;
            Ok(Some("recording started".into()))
        }
    }

    pub fn cancel(&mut self) -> bool {
        if let Some(handle) = self.recording.take() {
            handle.abort();
            self.state = DictationState::Idle;
            return true;
        }
        false
    }

    fn start_recording(&mut self, cfg: &DictationConfig) -> Result<(), String> {
        if self.recording.is_some() || self.state == DictationState::Transcribing {
            return Ok(());
        }
        let handle = RecordingHandle::start(&cfg.audio_source, cfg.recorder)?;
        self.recording = Some(handle);
        self.state = DictationState::Recording;
        self.last_error = None;
        Ok(())
    }

    fn finish_recording(&mut self, cfg: &DictationConfig) -> Result<Option<String>, String> {
        let Some(handle) = self.recording.take() else {
            return Ok(None);
        };
        let (wav_path, duration) = handle.stop()?;
        if duration.as_millis() < u128::from(cfg.min_duration_ms) {
            cleanup_wav(&wav_path);
            self.state = DictationState::Idle;
            return Ok(Some(format!(
                "ignored short tap ({}ms < {}ms)",
                duration.as_millis(),
                cfg.min_duration_ms
            )));
        }

        self.state = DictationState::Transcribing;
        let cfg_clone = cfg.clone();
        let tx = self.result_tx.clone();
        thread::spawn(move || {
            let text = stt::transcribe(&wav_path, &cfg_clone);
            cleanup_wav(&wav_path);
            let _ = tx.send(DictationResult { text, duration });
        });
        Ok(Some(format!(
            "transcribing {:.1}s of audio...",
            duration.as_secs_f64()
        )))
    }

    /// Polls for completed background transcription and injects text into the active window.
    pub fn poll_completed(
        &mut self,
        cfg: &DictationConfig,
        emitter: Option<&mut dyn InputEmitter>,
    ) -> Option<Result<String, String>> {
        let res = self.result_rx.try_recv().ok()?;
        match res.text {
            Ok(text) => {
                self.state = DictationState::Idle;
                if text.is_empty() {
                    return Some(Ok(String::new()));
                }
                self.last_transcript = Some(text.clone());
                let inject_cfg = TextInjectorConfig {
                    method: cfg.inject_method,
                    paste_shortcut: cfg.paste_shortcut,
                    restore_clipboard: cfg.restore_clipboard,
                    auto_enter: cfg.auto_enter,
                };
                if let Err(e) = inject_text(&text, &inject_cfg, emitter) {
                    self.last_error = Some(e.clone());
                    self.state = DictationState::Error;
                    if cfg.notify {
                        send_notification("open-au05 dictation error", &e);
                    }
                    return Some(Err(e));
                }
                if cfg.notify {
                    send_notification("open-au05 dictated", &text);
                }
                Some(Ok(text))
            }
            Err(e) => {
                self.last_error = Some(e.clone());
                self.state = DictationState::Error;
                if cfg.notify {
                    send_notification("open-au05 STT error", &e);
                }
                Some(Err(e))
            }
        }
    }
}

/// One-shot CLI helper: record for `seconds` (or until Ctrl+C), transcribe, and optionally inject.
pub fn run_oneshot_dictation(
    cfg: &DictationConfig,
    seconds: Option<u64>,
    inject: bool,
    emitter: Option<&mut dyn InputEmitter>,
) -> Result<(String, PathBuf), String> {
    let handle = RecordingHandle::start(&cfg.audio_source, cfg.recorder)?;
    eprintln!(
        "Recording from {} ({:?})... {}",
        handle.source_used.as_deref().unwrap_or("default"),
        handle.backend,
        match seconds {
            Some(s) => format!("for {s}s"),
            None => "press Enter to stop".into(),
        }
    );
    if let Some(s) = seconds {
        thread::sleep(Duration::from_secs(s));
    } else {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }
    let (wav_path, dur) = handle.stop()?;
    eprintln!("Captured {:.1}s, transcribing...", dur.as_secs_f64());
    let text = stt::transcribe(&wav_path, cfg)?;
    if inject && !text.is_empty() {
        let inject_cfg = TextInjectorConfig {
            method: cfg.inject_method,
            paste_shortcut: cfg.paste_shortcut,
            restore_clipboard: cfg.restore_clipboard,
            auto_enter: cfg.auto_enter,
        };
        inject_text(&text, &inject_cfg, emitter)?;
    }
    let saved_path = wav_path.clone();
    cleanup_wav(&wav_path);
    Ok((text, saved_path))
}

fn send_notification(title: &str, body: &str) {
    if command_exists("notify-send") {
        let _ = Command::new("notify-send")
            .args(["-a", "open-au05", "-t", "2500", title, body])
            .status();
    }
}
