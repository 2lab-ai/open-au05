//! Speech-to-Text backends:
//! 1. OpenAI-compatible `/v1/audio/transcriptions` multipart HTTP API (OpenAI, Groq, local whisper.cpp server, speaches, faster-whisper-server, llmux)
//! 2. Local CLI command template (`{wav}` placeholder)

use crate::config::{DictationConfig, SttBackend};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub fn transcribe(wav_path: &Path, cfg: &DictationConfig) -> Result<String, String> {
    match cfg.stt_backend {
        SttBackend::Openai => transcribe_openai_compatible(wav_path, cfg),
        SttBackend::Command => transcribe_command(wav_path, cfg),
    }
}

fn transcribe_openai_compatible(wav_path: &Path, cfg: &DictationConfig) -> Result<String, String> {
    let wav_bytes = fs::read(wav_path)
        .map_err(|e| format!("failed to read WAV {}: {e}", wav_path.display()))?;
    if wav_bytes.len() <= 44 {
        return Err("recorded audio is empty".into());
    }

    let boundary = format!("----open-au05-boundary-{}", std::process::id());
    let mut body = Vec::with_capacity(wav_bytes.len() + 1024);

    // model field
    push_text_field(&mut body, &boundary, "model", &cfg.model);
    if let Some(ref lang) = cfg.language {
        if !lang.trim().is_empty() {
            push_text_field(&mut body, &boundary, "language", lang.trim());
        }
    }
    if let Some(ref prompt) = cfg.prompt {
        if !prompt.trim().is_empty() {
            push_text_field(&mut body, &boundary, "prompt", prompt.trim());
        }
    }
    push_text_field(&mut body, &boundary, "response_format", "json");

    // audio file part
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: audio/wav\r\n\r\n");
    body.extend_from_slice(&wav_bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let mut req = ureq::post(&cfg.api_url)
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .timeout(Duration::from_secs(60));

    if !cfg.api_key_env.trim().is_empty() {
        if let Ok(key) = std::env::var(cfg.api_key_env.trim()) {
            if !key.trim().is_empty() {
                req = req.set("Authorization", &format!("Bearer {}", key.trim()));
            }
        }
    }

    let resp = req
        .send_bytes(&body)
        .map_err(|e| format!("STT API request to {} failed: {e}", cfg.api_url))?;
    let raw = resp
        .into_string()
        .map_err(|e| format!("failed to read STT response: {e}"))?;

    parse_stt_response(&raw)
}

pub fn parse_stt_response(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(text) = val.get("text").and_then(|v| v.as_str()) {
            return Ok(text.trim().to_string());
        }
    }
    Ok(trimmed.to_string())
}

fn transcribe_command(wav_path: &Path, cfg: &DictationConfig) -> Result<String, String> {
    let template = cfg
        .command
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            "stt_backend is 'command' but dictation.command is empty in config.toml".to_string()
        })?;
    let wav_quoted = shell_quote(&wav_path.to_string_lossy());
    let expanded = template.replace("{wav}", &wav_quoted);
    let output = Command::new("sh")
        .args(["-c", &expanded])
        .output()
        .map_err(|e| format!("failed to run STT command: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "STT command failed ({}): {}",
            output.status,
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn push_text_field(buf: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    buf.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    buf.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
    );
    buf.extend_from_slice(value.as_bytes());
    buf.extend_from_slice(b"\r\n");
}

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}
