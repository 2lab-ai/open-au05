//! Audio source auto-discovery (PipeWire/PulseAudio/ALSA) and WAV recording
//! targeting the AU05 USB microphone (`alsa_input.usb-AU05_*`).

use crate::config::RecorderBackend;
use crate::input::inject::command_exists;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct AudioSourceInfo {
    pub name: String,
    pub backend: String,
    pub is_au05: bool,
}

/// Discovers the AU05 microphone source on PipeWire / PulseAudio (`pactl`).
pub fn discover_au05_source() -> Option<AudioSourceInfo> {
    if command_exists("pactl") {
        if let Ok(out) = Command::new("pactl")
            .args(["list", "sources", "short"])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                for line in text.lines() {
                    let cols: Vec<&str> = line.split_whitespace().collect();
                    if cols.len() >= 2 {
                        let name = cols[1];
                        let lower = name.to_ascii_lowercase();
                        if (lower.contains("au05") || lower.contains("fff1"))
                            && !lower.ends_with(".monitor")
                        {
                            return Some(AudioSourceInfo {
                                name: name.to_string(),
                                backend: "pipewire/pulse".into(),
                                is_au05: true,
                            });
                        }
                    }
                }
            }
        }
    }
    None
}

pub fn resolve_recorder(preferred: RecorderBackend) -> Result<RecorderBackend, String> {
    match preferred {
        RecorderBackend::Auto => {
            if command_exists("pw-record") {
                Ok(RecorderBackend::PwRecord)
            } else if command_exists("parecord") {
                Ok(RecorderBackend::Parecord)
            } else if command_exists("arecord") {
                Ok(RecorderBackend::Arecord)
            } else if command_exists("ffmpeg") {
                Ok(RecorderBackend::Ffmpeg)
            } else {
                Err("no audio recorder found (install pipewire, pulseaudio-utils, alsa-utils, or ffmpeg)".into())
            }
        }
        other => Ok(other),
    }
}

pub struct RecordingHandle {
    child: Child,
    pub wav_path: PathBuf,
    pub started_at: Instant,
    pub source_used: Option<String>,
    pub backend: RecorderBackend,
}

impl RecordingHandle {
    pub fn start(audio_source: &str, preferred_backend: RecorderBackend) -> Result<Self, String> {
        let backend = resolve_recorder(preferred_backend)?;
        let source = if audio_source.eq_ignore_ascii_case("auto") || audio_source.is_empty() {
            discover_au05_source().map(|s| s.name)
        } else {
            Some(audio_source.to_string())
        };

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let wav_path =
            std::env::temp_dir().join(format!("open-au05-rec-{}-{stamp}.wav", std::process::id()));

        let mut cmd = match backend {
            RecorderBackend::PwRecord => {
                let mut c = Command::new("pw-record");
                c.args(["--format=s16", "--rate=16000", "--channels=1"]);
                if let Some(ref src) = source {
                    c.arg(format!("--target={src}"));
                }
                c.arg(&wav_path);
                c
            }
            RecorderBackend::Parecord => {
                let mut c = Command::new("parecord");
                c.args([
                    "--file-format=wav",
                    "--format=s16le",
                    "--rate=16000",
                    "--channels=1",
                ]);
                if let Some(ref src) = source {
                    c.args(["--device", src]);
                }
                c.arg(&wav_path);
                c
            }
            RecorderBackend::Arecord => {
                let mut c = Command::new("arecord");
                c.args(["-f", "S16_LE", "-r", "16000", "-c", "1", "-t", "wav"]);
                if let Some(ref src) = source {
                    if src.starts_with("hw:")
                        || src.starts_with("plughw:")
                        || src.starts_with("default")
                    {
                        c.args(["-D", src]);
                    }
                }
                c.arg(&wav_path);
                c
            }
            RecorderBackend::Ffmpeg => {
                let mut c = Command::new("ffmpeg");
                c.args(["-y", "-loglevel", "error", "-f", "pulse", "-i"]);
                c.arg(source.as_deref().unwrap_or("default"));
                c.args(["-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"]);
                c.arg(&wav_path);
                c
            }
            RecorderBackend::Auto => unreachable!(),
        };

        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("failed to start audio recorder ({backend:?}): {e}"))?;

        Ok(Self {
            child,
            wav_path,
            started_at: Instant::now(),
            source_used: source,
            backend,
        })
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Gracefully stops recording (SIGINT so WAV header is flushed cleanly) and returns the WAV path.
    pub fn stop(mut self) -> Result<(PathBuf, Duration), String> {
        let dur = self.started_at.elapsed();
        let pid = self.child.id() as libc::pid_t;
        unsafe {
            libc::kill(pid, libc::SIGINT);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(15));
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        if !self.wav_path.exists() {
            return Err("recording produced no WAV file".into());
        }
        Ok((self.wav_path, dur))
    }

    /// Aborts recording and removes the temporary WAV file.
    pub fn abort(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.wav_path);
    }
}

pub fn cleanup_wav(path: &Path) {
    let _ = std::fs::remove_file(path);
}
