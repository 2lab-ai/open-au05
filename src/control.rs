//! Unix domain socket IPC between the running `open-au05 daemon` and CLI / hook / TUI clients.

use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

pub fn socket_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("OPEN_AU05_SOCKET") {
        return PathBuf::from(explicit);
    }
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(runtime).join("open-au05.sock");
        if p.to_string_lossy().len() < 100 {
            return p;
        }
    }
    let uid = unsafe { libc::geteuid() };
    PathBuf::from(format!("/tmp/open-au05-{uid}.sock"))
}

pub struct ControlServer {
    listener: UnixListener,
    path: PathBuf,
}

impl ControlServer {
    pub fn bind() -> Result<Self, String> {
        let path = socket_path();
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let listener = UnixListener::bind(&path)
            .map_err(|e| format!("failed to bind control socket {}: {e}", path.display()))?;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("failed to set socket nonblocking: {e}"))?;
        Ok(Self { listener, path })
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Accepts and serves all pending client commands without blocking the daemon loop.
    pub fn poll<F>(&self, mut handler: F)
    where
        F: FnMut(Value) -> Value,
    {
        loop {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                    let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
                    let mut line = String::new();
                    let mut reader = BufReader::new(&stream);
                    if reader.read_line(&mut line).is_ok() && !line.trim().is_empty() {
                        let req: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
                        let resp = handler(req);
                        let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into());
                        out.push('\n');
                        let _ = stream.write_all(out.as_bytes());
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn send_command(payload: &Value) -> Result<Value, String> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path).map_err(|e| {
        format!(
            "cannot connect to open-au05 daemon at {} ({e}) — is `open-au05 daemon` running?",
            path.display()
        )
    })?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let mut raw = serde_json::to_string(payload).map_err(|e| e.to_string())?;
    raw.push('\n');
    stream
        .write_all(raw.as_bytes())
        .map_err(|e| format!("failed to write to daemon socket: {e}"))?;
    let mut reader = BufReader::new(&stream);
    let mut resp = String::new();
    reader
        .read_line(&mut resp)
        .map_err(|e| format!("failed to read daemon response: {e}"))?;
    serde_json::from_str(resp.trim()).map_err(|e| format!("invalid JSON response from daemon: {e}"))
}
