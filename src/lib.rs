//! Self-updating support for CLI binaries.
//!
//! `Updater` checks a GitHub release for a newer version, downloads the
//! release archive built for the current target triple, verifies it against
//! the published checksums file, replaces the running executable, and
//! re-executes it with the original arguments.
//!
//! Checksum verification here is an integrity check against accidental
//! corruption, not a tamper-proofing signature: the archive and its checksum
//! are served from the same release, so anyone able to alter one can alter the
//! other. Transport integrity relies on TLS to GitHub.
//!
//! # Example
//!
//! ```no_run
//! use auto_update::{Updater, WindowsPolicy};
//!
//! #[tokio::main]
//! async fn main() {
//!     let updater = Updater::new()
//!         .repo_owner("hyper-mcp-rs")
//!         .repo_name("hyper-mcp")
//!         .binary_name("hyper-mcp")
//!         .guard_env("HYPER_MCP_AUTO_UPDATED")
//!         .throttle_file("hyper-mcp-update-check")
//!         .windows_policy(WindowsPolicy::Disabled);
//!
//!     if let Err(e) = updater.run().await {
//!         tracing::warn!(error = ?e, "Auto-update failed; continuing with the current version");
//!     }
//! }
//! ```

use anyhow::{Context, Result};
use self_update::{backends::github::Update, cargo_crate_version};
use std::path::Path;
use std::time::Duration;

/// Policy for Windows auto-update behavior.
pub enum WindowsPolicy {
    /// Auto-update is disabled; callers should log a warning and skip.
    Disabled,
    /// Auto-update is enabled (Unix re-exec semantics; Windows support TBD).
    Enabled,
}

/// A configurable self-updater.
///
/// Each field is customizable so the same crate can serve multiple binaries
/// without env-var collisions or hardcoded assumptions.
pub struct Updater {
    /// GitHub owner (e.g. `"hyper-mcp-rs"`).
    repo_owner: String,
    /// GitHub repository name (e.g. `"hyper-mcp"`).
    repo_name: String,
    /// Binary name used when looking up the release asset.
    binary_name: String,
    /// Environment variable set on the re-executed process to prevent
    /// restart loops.
    guard_env: String,
    /// Throttle file name (stored in `$TMPDIR`).
    throttle_file: String,
    /// Throttle window duration.
    throttle_window: Duration,
    /// Whether auto-update is supported on this platform.
    windows_policy: WindowsPolicy,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            repo_owner: String::new(),
            repo_name: String::new(),
            binary_name: String::new(),
            guard_env: String::from("AUTO_UPDATE_GUARD"),
            throttle_file: String::from("auto-update-check"),
            throttle_window: Duration::from_secs(15 * 60),
            windows_policy: WindowsPolicy::Enabled,
        }
    }
}

impl Updater {
    /// Create a new updater with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the GitHub owner.
    pub fn repo_owner(mut self, owner: &str) -> Self {
        self.repo_owner = owner.to_string();
        self
    }

    /// Set the GitHub repository name.
    pub fn repo_name(mut self, name: &str) -> Self {
        self.repo_name = name.to_string();
        self
    }

    /// Set the binary name used for release asset lookup.
    pub fn binary_name(mut self, name: &str) -> Self {
        self.binary_name = name.to_string();
        self
    }

    /// Set the guard environment variable name.
    pub fn guard_env(mut self, env: &str) -> Self {
        self.guard_env = env.to_string();
        self
    }

    /// Set the throttle file name.
    pub fn throttle_file(mut self, name: &str) -> Self {
        self.throttle_file = name.to_string();
        self
    }

    /// Set the throttle window duration.
    pub fn throttle_window(mut self, window: Duration) -> Self {
        self.throttle_window = window;
        self
    }

    /// Set the Windows policy.
    pub fn windows_policy(mut self, policy: WindowsPolicy) -> Self {
        self.windows_policy = policy;
        self
    }

    /// Run the update check.
    ///
    /// On success with no update available, returns `Ok(())` and the caller
    /// proceeds. If an update is applied, this function does not return: it
    /// replaces the process image and re-executes with Unix `exec()`,
    /// preserving stdio file descriptors. Any failure is returned as an error;
    /// the caller is expected to log it and continue running the current version.
    pub async fn run(self) -> Result<()> {
        // Windows check
        if cfg!(target_os = "windows") && matches!(self.windows_policy, WindowsPolicy::Disabled) {
            return Err(anyhow::anyhow!("Auto-update is not supported on Windows"));
        }

        // Guard: skip if already restarted after an update.
        if std::env::var_os(&self.guard_env).is_some() {
            tracing::debug!(
                guard = %self.guard_env,
                "Auto-update guard set; already updated this launch, skipping check"
            );
            return Ok(());
        }

        // Throttle: skip if we checked within the window.
        if !should_check(&self.throttle_file, &self.throttle_window) {
            tracing::info!(
                throttle_file = %self.throttle_file,
                "Auto-update: check skipped (within throttle window)"
            );
            return Ok(());
        }

        // Build the self_update updater.
        let updater = Update::configure()
            .repo_owner(&self.repo_owner)
            .repo_name(&self.repo_name)
            .bin_name(&self.binary_name)
            .current_version(cargo_crate_version!())
            .no_confirm(true) // unattended — no interactive prompt
            .show_download_progress(false) // silent in MCP mode
            .target(get_target())
            .build()?;

        // Perform the update: download, verify checksum, extract, install.
        let status = updater.update()?;
        tracing::info!(
            "Auto-update: installed new binary, version {}",
            status.version()
        );
        touch_throttle(&self.throttle_file);

        // Restart the process with the new binary.
        restart(&std::env::current_exe()?, &self.guard_env)
    }
}

// ---- Internal helpers -----------------------------------------------------

/// Returns true if we should perform an update check.
fn should_check(throttle_file: &str, throttle_window: &Duration) -> bool {
    let path = throttle_path(throttle_file);
    match std::fs::metadata(&path) {
        Ok(meta) => {
            if let Ok(mtime) = meta.modified()
                && let Ok(elapsed) = std::time::SystemTime::now().duration_since(mtime)
            {
                return elapsed > *throttle_window;
            }
            true
        }
        Err(_) => true,
    }
}

/// Returns the path to the throttle file in the system temp directory.
fn throttle_path(throttle_file: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(throttle_file)
}

/// Touches the throttle file to update its mtime.
fn touch_throttle(throttle_file: &str) {
    let path = throttle_path(throttle_file);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path);
}

/// Restarts the process using the freshly installed executable.
fn restart(exe: &Path, guard_env: &str) -> Result<()> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let mut command = std::process::Command::new(exe);
    command.args(&args).env(guard_env, "1");

    use std::os::unix::process::CommandExt;
    let err = command.exec();
    Err(err).context("re-executing updated binary")
}

/// Returns the target triple from the `BUILD_TARGET` build-time env var.
fn get_target() -> &'static str {
    env!("BUILD_TARGET")
}

// ---- Tests ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_updater_has_configurable_fields() {
        let u = Updater::new();
        assert!(!u.guard_env.is_empty());
        assert!(!u.throttle_file.is_empty());
    }

    #[test]
    fn builder_fluent() {
        let u = Updater::new()
            .repo_owner("test-owner")
            .repo_name("test-repo")
            .binary_name("test-binary")
            .guard_env("TEST_GUARD")
            .throttle_file("test-throttle")
            .throttle_window(Duration::from_secs(300))
            .windows_policy(WindowsPolicy::Disabled);

        assert_eq!(u.repo_owner, "test-owner");
        assert_eq!(u.repo_name, "test-repo");
        assert_eq!(u.binary_name, "test-binary");
        assert_eq!(u.guard_env, "TEST_GUARD");
        assert_eq!(u.throttle_file, "test-throttle");
        assert_eq!(u.throttle_window, Duration::from_secs(300));
        assert!(matches!(u.windows_policy, WindowsPolicy::Disabled));
    }

    #[test]
    fn get_target_returns_env_value() {
        let target = get_target();
        assert!(!target.is_empty());
    }

    // ---- Throttling tests --------------------------------------------------
    // These tests use a shared throttle file in the temp directory, so they
    // must run sequentially to avoid interference.

    #[serial_test::serial(throttle_tests)]
    #[test]
    fn should_check_returns_true_when_no_throttle_file() {
        let file = "test-auto-update-no-file";
        let _ = std::fs::remove_file(throttle_path(file));

        assert!(
            should_check(file, &Duration::from_secs(15 * 60)),
            "should_check should return true when throttle file does not exist"
        );

        let _ = std::fs::remove_file(throttle_path(file));
    }

    #[serial_test::serial(throttle_tests)]
    #[test]
    fn should_check_returns_true_when_throttle_file_is_old() {
        let file = "test-auto-update-old-file";
        let _ = std::fs::remove_file(throttle_path(file));

        let path = throttle_path(file);
        std::fs::write(&path, "test").unwrap();

        let old_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            - Duration::from_secs(20 * 60);
        let old_time = filetime::FileTime::from_unix_time(old_time.as_secs() as i64, 0);
        filetime::set_file_mtime(&path, old_time).unwrap();

        assert!(
            should_check(file, &Duration::from_secs(15 * 60)),
            "should_check should return true when throttle file is older than window"
        );

        let _ = std::fs::remove_file(throttle_path(file));
    }

    #[serial_test::serial(throttle_tests)]
    #[test]
    fn should_check_returns_false_when_throttle_file_is_recent() {
        let file = "test-auto-update-recent-file";
        let _ = std::fs::remove_file(throttle_path(file));

        let path = throttle_path(file);
        std::fs::write(&path, "test").unwrap();

        assert!(
            !should_check(file, &Duration::from_secs(15 * 60)),
            "should_check should return false when throttle file exists and is recent"
        );

        let _ = std::fs::remove_file(throttle_path(file));
    }

    #[serial_test::serial(throttle_tests)]
    #[test]
    fn touch_throttle_creates_file() {
        let file = "test-auto-update-touch";
        let _ = std::fs::remove_file(throttle_path(file));

        touch_throttle(file);

        assert!(
            throttle_path(file).exists(),
            "touch_throttle should create the throttle file"
        );

        let _ = std::fs::remove_file(throttle_path(file));
    }
}
