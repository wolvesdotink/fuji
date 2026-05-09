use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Result of a PTP scan — list of cameras.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct PtpCameraInfo {
    pub name: String,
    pub serial: String,
    pub model: String,
}

/// Diagnostic snapshot of the PTP sidecar — what we see without interpreting.
/// Used by the UI "Run Diagnostics" panel to explain why detection is failing.
#[derive(Debug, Serialize)]
pub struct PtpDiagnostics {
    pub binary_path: String,
    pub binary_exists: bool,
    pub scan_stdout: String,
    pub scan_stderr: String,
    pub invocation_error: Option<String>,
}

/// A single file entry from the PTP catalog.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct PtpFileEntry {
    pub name: String,
    pub size: i64,
    pub uti: String,
    pub folder: String,
    pub thumbnail: Option<String>,
}

/// Result of a PTP catalog command.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct PtpCatalogResult {
    pub camera: String,
    pub files: Vec<PtpFileEntry>,
}

/// Result of a PTP download command.
#[derive(Debug, Deserialize)]
pub struct PtpDownloadResult {
    pub downloaded: Vec<PtpDownloadedFile>,
    pub errors: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PtpDownloadedFile {
    pub name: String,
    pub path: String,
}

/// Result of a PTP delete command.
#[derive(Debug, Deserialize)]
pub struct PtpDeleteResult {
    pub deleted: u32,
    pub errors: Vec<String>,
}

/// Error wrapper for one-shot PTP bridge responses (used by `diagnose()` only).
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PtpError {
    error: String,
}

/// Find the ptp-bridge binary.
/// During development it's in src-tauri/binaries/, in production it's bundled as a sidecar.
fn ptp_bridge_path() -> PathBuf {
    // First check next to the current executable (sidecar location in production)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sidecar = dir.join("ptp-bridge");
            if sidecar.exists() {
                return sidecar;
            }
        }
    }

    // During development, check the binaries directory relative to the manifest
    let dev_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(format!("ptp-bridge-{}", env!("TARGET_TRIPLE")));
    if dev_path.exists() {
        return dev_path;
    }

    // Fallback: just try "ptp-bridge" in PATH
    PathBuf::from("ptp-bridge")
}

// =============================================================================
// Persistent PtpBridge
// =============================================================================

/// A long-lived `ptp-bridge daemon` child process. The daemon keeps a single
/// `ICDeviceBrowser` alive across all commands, which eliminates the
/// re-discovery race that caused "Camera not found" errors on catalog after a
/// successful scan.
///
/// Lifecycle:
/// - One instance per app (managed by Tauri state).
/// - Child spawned lazily on first request; respawned if it exits unexpectedly.
/// - `shutdown()` (or `Drop`) gracefully stops the child.
///
/// Concurrency:
/// - Request ids are monotonic; responses are correlated by id.
/// - A dedicated reader thread parses NDJSON responses from the child's stdout
///   and routes them to the waiting caller via oneshot `mpsc` channels.
/// - Writes to the child's stdin are serialized by the bridge's internal mutex.
pub struct PtpBridge {
    state: Arc<Mutex<BridgeState>>,
}

struct BridgeState {
    /// Handle to the running daemon. `None` when not spawned or after exit.
    handle: Option<DaemonHandle>,
    /// Monotonic request id counter.
    next_id: u64,
    /// Pending requests awaiting a response, keyed by id.
    pending: HashMap<u64, mpsc::Sender<Value>>,
}

struct DaemonHandle {
    child: Child,
    stdin: BufWriter<ChildStdin>,
}

impl PtpBridge {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(BridgeState {
                handle: None,
                next_id: 1,
                pending: HashMap::new(),
            })),
        }
    }

    /// Send a command to the daemon and await its response.
    fn request(&self, cmd: &str, args: Value, timeout: Duration) -> Result<Value, String> {
        // Make sure we have a live daemon. May spawn a new one if we don't.
        self.ensure_daemon()?;

        // Register + write request under the state lock.
        //
        // Ordering matters: state access happens in short-lived borrows, and
        // the mutable borrow of `state.handle` is taken LAST so it doesn't
        // conflict with `state.next_id` / `state.pending` access (which all go
        // through the MutexGuard's DerefMut and therefore can't split-borrow
        // across the guard).
        let (tx, rx) = mpsc::channel::<Value>();
        let id = {
            let mut state = self
                .state
                .lock()
                .map_err(|e| format!("bridge state lock poisoned: {}", e))?;

            // Fail early if the daemon isn't alive — avoids a half-registered
            // pending entry.
            if state.handle.is_none() {
                return Err(
                    "ptp-bridge daemon exited before request could be sent".to_string(),
                );
            }

            let id = state.next_id;
            state.next_id = id.wrapping_add(1).max(1);

            // Build the request line — pure compute, no state access.
            let mut req = match args {
                Value::Object(m) => m,
                _ => serde_json::Map::new(),
            };
            req.insert("id".into(), json!(id));
            req.insert("cmd".into(), json!(cmd));
            let line = serde_json::to_string(&Value::Object(req))
                .map_err(|e| format!("failed to serialize request: {}", e))?;

            // Register the pending responder BEFORE writing, so the reader
            // thread never delivers a response for an id nobody's waiting on.
            state.pending.insert(id, tx);

            // Finally, acquire the handle and perform the stdin write. Nothing
            // else touches state after this point within the block, so the
            // mutable borrow doesn't conflict with anything.
            let handle = state.handle.as_mut().expect("handle presence checked above");
            if let Err(e) = writeln!(handle.stdin, "{}", line) {
                state.pending.remove(&id);
                return Err(format!("failed to write to ptp-bridge stdin: {}", e));
            }
            if let Err(e) = handle.stdin.flush() {
                state.pending.remove(&id);
                return Err(format!("failed to flush ptp-bridge stdin: {}", e));
            }

            id
        };

        log::debug!("ptp-bridge -> id={} cmd={}", id, cmd);

        match rx.recv_timeout(timeout) {
            Ok(response) => {
                log::debug!("ptp-bridge <- id={}", id);
                parse_response(response)
            }
            Err(_) => {
                // Either timed out or the sender was dropped (daemon died and
                // the reader thread cleaned up pending). Remove any lingering
                // pending entry and report.
                if let Ok(mut state) = self.state.lock() {
                    state.pending.remove(&id);
                }
                Err(format!(
                    "ptp-bridge request timed out after {}s (cmd={})",
                    timeout.as_secs(),
                    cmd
                ))
            }
        }
    }

    /// Ensure there is a live daemon. Spawns one if `handle` is None.
    fn ensure_daemon(&self) -> Result<(), String> {
        let need_spawn = {
            let state = self
                .state
                .lock()
                .map_err(|e| format!("bridge state lock poisoned: {}", e))?;
            state.handle.is_none()
        };
        if need_spawn {
            self.spawn_daemon()?;
        }
        Ok(())
    }

    fn spawn_daemon(&self) -> Result<(), String> {
        let binary = ptp_bridge_path();
        log::info!("Spawning ptp-bridge daemon: {:?}", binary);

        let mut child = Command::new(&binary)
            .arg("daemon")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                format!(
                    "Failed to spawn ptp-bridge daemon: {} (path: {:?})",
                    e, binary
                )
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ptp-bridge daemon has no stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ptp-bridge daemon has no stdout".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "ptp-bridge daemon has no stderr".to_string())?;

        // Install the handle. If another thread already installed one, kill
        // our newly-spawned child and drop it — we lost the race but that's
        // fine, the winner will serve our request.
        {
            let mut state = self
                .state
                .lock()
                .map_err(|e| format!("bridge state lock poisoned: {}", e))?;
            if state.handle.is_some() {
                log::info!("Another thread already spawned the daemon; discarding ours");
                let _ = child.kill();
                let _ = child.wait();
                return Ok(());
            }
            state.handle = Some(DaemonHandle {
                child,
                stdin: BufWriter::new(stdin),
            });
        }

        // Reader thread: demultiplex NDJSON responses to pending senders.
        let reader_state = self.state.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line_res in reader.lines() {
                let line = match line_res {
                    Ok(l) => l,
                    Err(e) => {
                        log::warn!("ptp-bridge: stdout read error: {}", e);
                        break;
                    }
                };
                if line.is_empty() {
                    continue;
                }
                let response: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(e) => {
                        log::warn!(
                            "ptp-bridge: invalid JSON response (ignoring): {} (err: {})",
                            line,
                            e
                        );
                        continue;
                    }
                };
                let id = response.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                let tx = match reader_state.lock() {
                    Ok(mut s) => s.pending.remove(&id),
                    Err(e) => {
                        log::error!("ptp-bridge: state lock poisoned in reader: {}", e);
                        break;
                    }
                };
                if let Some(tx) = tx {
                    let _ = tx.send(response);
                } else if id != 0 {
                    log::warn!(
                        "ptp-bridge: response for unknown id={} (possibly a duplicate or late)",
                        id
                    );
                }
            }

            // EOF or read error → daemon exited. Clear the handle (so the next
            // request respawns) and notify all in-flight callers.
            log::info!("ptp-bridge: daemon stdout closed, marking it as exited");
            if let Ok(mut state) = reader_state.lock() {
                if let Some(mut handle) = state.handle.take() {
                    // Reap the zombie.
                    let _ = handle.child.wait();
                }
                let pending: Vec<_> = state.pending.drain().collect();
                drop(state);
                for (_, tx) in pending {
                    let _ = tx.send(json!({
                        "ok": false,
                        "error": "ptp-bridge daemon exited"
                    }));
                }
            }
        });

        // Stderr forwarder: funnel bridge logs into our app log.
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                log::info!("ptp-bridge: {}", line);
            }
        });

        Ok(())
    }

    pub fn scan(&self) -> Result<Vec<PtpCameraInfo>, String> {
        let result = self.request("scan", json!({}), Duration::from_secs(15))?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse scan result: {}", e))
    }

    pub fn catalog(
        &self,
        camera_name: &str,
        thumb_cache_dir: &str,
    ) -> Result<PtpCatalogResult, String> {
        let result = self.request(
            "catalog",
            json!({
                "camera": camera_name,
                "thumb_dir": thumb_cache_dir,
            }),
            Duration::from_secs(300),
        )?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse catalog result: {}", e))
    }

    pub fn download(
        &self,
        camera_name: &str,
        dest_dir: &str,
        file_names: &[String],
    ) -> Result<PtpDownloadResult, String> {
        let result = self.request(
            "download",
            json!({
                "camera": camera_name,
                "dest_dir": dest_dir,
                "files": file_names,
            }),
            Duration::from_secs(3600),
        )?;
        serde_json::from_value(result)
            .map_err(|e| format!("Failed to parse download result: {}", e))
    }

    pub fn delete(
        &self,
        camera_name: &str,
        file_names: &[String],
    ) -> Result<PtpDeleteResult, String> {
        let result = self.request(
            "delete",
            json!({
                "camera": camera_name,
                "files": file_names,
            }),
            Duration::from_secs(120),
        )?;
        serde_json::from_value(result).map_err(|e| format!("Failed to parse delete result: {}", e))
    }

    /// Ask the daemon to exit gracefully and reap the child.
    pub fn shutdown(&self) {
        let handle_opt = {
            match self.state.lock() {
                Ok(mut state) => state.handle.take(),
                Err(_) => return,
            }
        };
        if let Some(mut handle) = handle_opt {
            let _ = writeln!(handle.stdin, r#"{{"id":0,"cmd":"shutdown"}}"#);
            let _ = handle.stdin.flush();
            // Dropping stdin closes it, which the daemon treats as EOF.
            drop(handle.stdin);
            // Give it a brief moment, then force-kill if needed.
            thread::sleep(Duration::from_millis(200));
            let _ = handle.child.kill();
            let _ = handle.child.wait();
        }
    }
}

impl Default for PtpBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PtpBridge {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Parse a daemon response into Ok(result) / Err(error).
fn parse_response(v: Value) -> Result<Value, String> {
    let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
    if ok {
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    } else {
        let err = v
            .get("error")
            .and_then(|x| x.as_str())
            .unwrap_or("unknown error")
            .to_string();
        Err(err)
    }
}

// =============================================================================
// One-shot diagnostics (does NOT go through the daemon)
// =============================================================================

/// Run a diagnostic scan as a fresh one-shot process, capturing stdout and
/// stderr verbatim. Intentionally bypasses the persistent daemon so the user
/// can debug even when the daemon itself is broken.
pub fn diagnose() -> PtpDiagnostics {
    let binary = ptp_bridge_path();
    let binary_path = binary.to_string_lossy().to_string();
    let binary_exists = binary.exists();

    if !binary_exists {
        let err = format!("Binary not found at {}", binary_path);
        return PtpDiagnostics {
            binary_path,
            binary_exists: false,
            scan_stdout: String::new(),
            scan_stderr: String::new(),
            invocation_error: Some(err),
        };
    }

    match Command::new(&binary).args(["scan"]).output() {
        Ok(output) => PtpDiagnostics {
            binary_path,
            binary_exists: true,
            scan_stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            scan_stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            invocation_error: None,
        },
        Err(e) => PtpDiagnostics {
            binary_path,
            binary_exists: true,
            scan_stdout: String::new(),
            scan_stderr: String::new(),
            invocation_error: Some(format!("Failed to spawn ptp-bridge: {}", e)),
        },
    }
}

// =============================================================================
// Pure helpers
// =============================================================================

/// Parse a PTP path like "ptp://CameraName/DSCF1234.HIF" into (camera_name, file_name).
pub fn parse_ptp_path(path: &str) -> Option<(String, String)> {
    let stripped = path.strip_prefix("ptp://")?;
    let slash_idx = stripped.find('/')?;
    let camera = stripped[..slash_idx].to_string();
    let file = stripped[slash_idx + 1..].to_string();
    Some((camera, file))
}

/// Build a PTP path from camera name and file name.
pub fn make_ptp_path(camera_name: &str, file_name: &str) -> String {
    format!("ptp://{}/{}", camera_name, file_name)
}
