//! Helpers shared by the unit tests.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A directory removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        let unique = format!(
            "wlr-monitor-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        let path = std::env::temp_dir().join(unique);
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        // Resolve /tmp -> /private/tmp on macOS so paths compare equal.
        Self(path.canonicalize().unwrap())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, part: impl AsRef<Path>) -> PathBuf {
        self.0.join(part)
    }

    /// Write an executable shell script and return its path.
    pub fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Shell-quote one argument.
pub fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::model::tests::load_outputs;
use crate::model::{Output, OutputState};
use crate::wlr::{ApplyResult, Wlr, WlrRandrError};

/// wlr-randr stand-in that records applies.
#[derive(Clone, Default)]
pub struct FakeWlr {
    pub outputs: Arc<Mutex<Vec<Output>>>,
    /// Re-read the fixture on every read (so compositor-only settings are
    /// unknown again), keeping only the positions of `outputs`.
    pub from_fixture: bool,
    /// Applies change what the next read returns, like a compositor.
    pub live: bool,
    pub read_error: Option<String>,
    pub applied: Arc<Mutex<Vec<OutputState>>>,
    pub reads: Arc<Mutex<usize>>,
    /// Block reads until this file exists (to prove queries run in parallel).
    pub wait_for: Option<PathBuf>,
    /// Apply results by output name: `false` fails that output.
    pub fail: Vec<String>,
    /// Appended to every apply as a `config` warning.
    pub warning: Option<String>,
}

impl FakeWlr {
    /// Like the real tool: positions survive, compositor-only settings don't.
    pub fn new(outputs: Vec<Output>) -> Self {
        Self {
            outputs: Arc::new(Mutex::new(outputs)),
            from_fixture: true,
            ..Self::default()
        }
    }

    /// Returns exactly `outputs` on every read.
    pub fn exact(outputs: Vec<Output>) -> Self {
        Self {
            outputs: Arc::new(Mutex::new(outputs)),
            ..Self::default()
        }
    }

    /// Returns `outputs`, updated by every apply.
    pub fn live(outputs: Vec<Output>) -> Self {
        Self {
            live: true,
            ..Self::exact(outputs)
        }
    }

    pub fn set_outputs(&self, outputs: Vec<Output>) {
        *self.outputs.lock().unwrap() = outputs;
    }

    pub fn failing(message: &str) -> Self {
        Self {
            read_error: Some(message.to_string()),
            ..Self::default()
        }
    }

    pub fn applied(&self) -> Vec<String> {
        self.applied
            .lock()
            .unwrap()
            .iter()
            .map(|state| state.name.clone())
            .collect()
    }

    pub fn applied_states(&self) -> Vec<OutputState> {
        self.applied.lock().unwrap().clone()
    }

    pub fn read_count(&self) -> usize {
        *self.reads.lock().unwrap()
    }
}

impl Wlr for FakeWlr {
    fn read_outputs(&self) -> Result<Vec<Output>, WlrRandrError> {
        *self.reads.lock().unwrap() += 1;
        if let Some(message) = &self.read_error {
            return Err(WlrRandrError(message.clone()));
        }
        if let Some(marker) = &self.wait_for {
            let started = Instant::now();
            while !marker.exists() {
                assert!(
                    started.elapsed() < Duration::from_secs(5),
                    "IPC query did not run in parallel"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        if !self.from_fixture {
            return Ok(self.outputs.lock().unwrap().clone());
        }
        let fixture = load_outputs();
        Ok(self
            .outputs
            .lock()
            .unwrap()
            .iter()
            .map(|output| {
                let mut fresh = fixture
                    .iter()
                    .find(|item| item.name == output.name)
                    .cloned()
                    .unwrap();
                fresh.x = output.x;
                fresh.y = output.y;
                fresh
            })
            .collect())
    }

    fn apply_states(&self, states: &[OutputState]) -> Result<Vec<ApplyResult>, WlrRandrError> {
        let mut applied = self.applied.lock().unwrap();
        let mut outputs = self.outputs.lock().unwrap();
        let mut results: Vec<ApplyResult> = states
            .iter()
            .map(|state| {
                applied.push(state.clone());
                if self.fail.contains(&state.name) {
                    return ApplyResult::failure(state.name.clone(), Vec::new(), "bad mode");
                }
                if self.live
                    && let Some(output) =
                        outputs.iter_mut().find(|output| output.name == state.name)
                {
                    state.apply_to(output, false);
                }
                ApplyResult::success(state.name.clone(), Vec::new())
            })
            .collect();
        if let Some(warning) = &self.warning {
            results.push(ApplyResult {
                warning: warning.clone(),
                ..ApplyResult::success("config", Vec::new())
            });
        }
        Ok(results)
    }
}

/// An in-memory `Write` that can be read back.
#[derive(Clone, Default)]
pub struct Buffer(pub Arc<Mutex<Vec<u8>>>);

impl Buffer {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }

    pub fn take(&self) -> String {
        let text = self.text();
        self.0.lock().unwrap().clear();
        text
    }
}

impl std::io::Write for Buffer {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
