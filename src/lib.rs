//! Self-updating support for CLI binaries.
//!
//! This crate provides two composable wrappers around any type implementing
//! [`self_update::update::ReleaseUpdate`]:
//!
//! - [`ThrottledUpdate`] limits how often update checks run by recording the
//!   time of the last check in a throttle file in the system temp directory.
//! - [`RestartUpdate`] re-executes the process with the freshly installed
//!   binary after a successful update, using a guard environment variable to
//!   prevent restart loops.
//!
//! Both wrappers implement `ReleaseUpdate` themselves, so they can be layered
//! over a backend (or over each other) and used anywhere a `ReleaseUpdate` is
//! expected.
//!
//! # Example
//!
//! ```ignore
//! use auto_update::{RestartUpdate, ThrottledUpdate};
//! use self_update::update::ReleaseUpdate;
//! use std::time::Duration;
//!
//! // `backend` is any `ReleaseUpdate` implementation, e.g. a self_update
//! // GitHub backend.
//! let updater = RestartUpdate::new(
//!     ThrottledUpdate::new(backend).throttle_window(Duration::from_secs(15 * 60)),
//! )
//! .guard_env("HYPER_MCP_AUTO_UPDATED");
//!
//! let status = updater.update()?;
//! ```

use self_update::Status;
use self_update::errors::Result;
use self_update::update::{Release, ReleaseUpdate, UpdateStatus};
use std::time::Duration;

/// Wraps a [`ReleaseUpdate`] and restarts the process with the newly
/// installed binary after a successful update.
pub struct RestartUpdate<R: ReleaseUpdate> {
    inner: R,
    /// Environment variable set on the re-executed process to prevent
    /// restart loops.
    guard_env: String,
}

impl<R: ReleaseUpdate> RestartUpdate<R> {
    /// Create a new updater wrapping the given release update implementation.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            guard_env: String::from("AUTO_UPDATE_GUARD"),
        }
    }

    /// Set the guard environment variable name.
    pub fn guard_env(mut self, env: &str) -> Self {
        self.guard_env = env.to_string();
        self
    }

    /// Restart the process using the freshly installed executable.
    fn restart(&self) -> Result<()> {
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        let mut command = std::process::Command::new(self.inner.bin_install_path());
        command.args(&args).env(self.guard_env.clone(), "1");

        use std::os::unix::process::CommandExt;
        let err = command.exec();
        Err(self_update::errors::Error::Release(format!(
            "re-executing updated binary: {}",
            err
        )))
    }
}

impl<R: ReleaseUpdate> ReleaseUpdate for RestartUpdate<R> {
    fn get_latest_release(&self) -> Result<Release> {
        self.inner.get_latest_release()
    }

    fn get_latest_releases(&self, current_version: &str) -> Result<Vec<Release>> {
        self.inner.get_latest_releases(current_version)
    }

    fn get_release_version(&self, ver: &str) -> Result<Release> {
        self.inner.get_release_version(ver)
    }

    fn current_version(&self) -> String {
        self.inner.current_version()
    }

    fn target(&self) -> String {
        self.inner.target()
    }

    fn target_version(&self) -> Option<String> {
        self.inner.target_version()
    }

    fn bin_name(&self) -> String {
        self.inner.bin_name()
    }

    fn bin_install_path(&self) -> std::path::PathBuf {
        self.inner.bin_install_path()
    }

    fn bin_path_in_archive(&self) -> String {
        self.inner.bin_path_in_archive()
    }

    fn show_download_progress(&self) -> bool {
        self.inner.show_download_progress()
    }

    fn show_output(&self) -> bool {
        self.inner.show_output()
    }

    fn no_confirm(&self) -> bool {
        self.inner.no_confirm()
    }

    fn progress_template(&self) -> String {
        self.inner.progress_template()
    }

    fn progress_chars(&self) -> String {
        self.inner.progress_chars()
    }

    fn auth_token(&self) -> Option<String> {
        self.inner.auth_token()
    }

    fn update(&self) -> Result<Status> {
        // Already restarted after an update: nothing left to do.
        if std::env::var_os(&self.guard_env).is_some() {
            return Ok(Status::UpToDate(self.current_version()));
        }

        let status = self.inner.update()?;
        if let Status::Updated(_) = status {
            self.restart()?;
        }
        Ok(status)
    }

    fn update_extended(&self) -> Result<UpdateStatus> {
        // Already restarted after an update: nothing left to do.
        if std::env::var_os(&self.guard_env).is_some() {
            return Ok(UpdateStatus::UpToDate);
        }

        let status = self.inner.update_extended()?;
        if let UpdateStatus::Updated(_) = status {
            self.restart()?;
        }
        Ok(status)
    }
}

/// Wraps a [`ReleaseUpdate`] and throttles how often update checks run.
///
/// The underlying update check is skipped when one was already performed
/// within `throttle_window`. The time of the last check is tracked via a
/// throttle file in the system temp directory.
pub struct ThrottledUpdate<R: ReleaseUpdate> {
    inner: R,
    /// Minimum time between update checks. Defaults to 15 minutes.
    throttle_window: Duration,
}

impl<R: ReleaseUpdate> ThrottledUpdate<R> {
    /// Create a new updater wrapping the given release update implementation.
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            throttle_window: Duration::from_secs(15 * 60),
        }
    }

    /// Set the throttle window duration.
    pub fn throttle_window(mut self, window: Duration) -> Self {
        self.throttle_window = window;
        self
    }

    /// Returns true if enough time has elapsed to perform an update check.
    fn should_check(&self) -> bool {
        let path = self.throttle_path();
        match std::fs::metadata(&path) {
            Ok(meta) => {
                if let Ok(mtime) = meta.modified()
                    && let Ok(elapsed) = std::time::SystemTime::now().duration_since(mtime)
                {
                    return elapsed > self.throttle_window;
                }
                true
            }
            Err(_) => true,
        }
    }

    /// Path to the throttle file in the system temp directory.
    fn throttle_path(&self) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("{}.throttle", &self.inner.bin_name()))
    }

    /// Update the throttle file's mtime, creating it if necessary.
    fn touch_throttle(&self) {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(self.throttle_path());
    }
}

impl<R: ReleaseUpdate> ReleaseUpdate for ThrottledUpdate<R> {
    fn get_latest_release(&self) -> Result<Release> {
        self.inner.get_latest_release()
    }

    fn get_latest_releases(&self, current_version: &str) -> Result<Vec<Release>> {
        self.inner.get_latest_releases(current_version)
    }

    fn get_release_version(&self, ver: &str) -> Result<Release> {
        self.inner.get_release_version(ver)
    }

    fn current_version(&self) -> String {
        self.inner.current_version()
    }

    fn target(&self) -> String {
        self.inner.target()
    }

    fn target_version(&self) -> Option<String> {
        self.inner.target_version()
    }

    fn bin_name(&self) -> String {
        self.inner.bin_name()
    }

    fn bin_install_path(&self) -> std::path::PathBuf {
        self.inner.bin_install_path()
    }

    fn bin_path_in_archive(&self) -> String {
        self.inner.bin_path_in_archive()
    }

    fn show_download_progress(&self) -> bool {
        self.inner.show_download_progress()
    }

    fn show_output(&self) -> bool {
        self.inner.show_output()
    }

    fn no_confirm(&self) -> bool {
        self.inner.no_confirm()
    }

    fn progress_template(&self) -> String {
        self.inner.progress_template()
    }

    fn progress_chars(&self) -> String {
        self.inner.progress_chars()
    }

    fn auth_token(&self) -> Option<String> {
        self.inner.auth_token()
    }

    fn update(&self) -> Result<Status> {
        if !self.should_check() {
            return Ok(Status::UpToDate(self.current_version()));
        }

        let result = self.inner.update();
        self.touch_throttle();
        result
    }

    fn update_extended(&self) -> Result<UpdateStatus> {
        if !self.should_check() {
            return Ok(UpdateStatus::UpToDate);
        }

        let result = self.inner.update_extended();
        self.touch_throttle();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A minimal [`ReleaseUpdate`] used to observe how the wrappers drive the
    /// underlying backend without performing any real network or file work.
    struct MockRelease {
        bin_name: String,
        update_calls: Cell<usize>,
        report_updated: bool,
    }

    impl MockRelease {
        fn new(bin_name: &str) -> Self {
            Self {
                bin_name: bin_name.to_string(),
                update_calls: Cell::new(0),
                report_updated: false,
            }
        }

        fn calls(&self) -> usize {
            self.update_calls.get()
        }
    }

    impl ReleaseUpdate for MockRelease {
        fn get_latest_release(&self) -> Result<Release> {
            Ok(Release::default())
        }

        fn get_latest_releases(&self, _current_version: &str) -> Result<Vec<Release>> {
            Ok(Vec::new())
        }

        fn get_release_version(&self, _ver: &str) -> Result<Release> {
            Ok(Release::default())
        }

        fn current_version(&self) -> String {
            "1.0.0".to_string()
        }

        fn target(&self) -> String {
            "test-target".to_string()
        }

        fn target_version(&self) -> Option<String> {
            None
        }

        fn bin_name(&self) -> String {
            self.bin_name.clone()
        }

        fn bin_install_path(&self) -> std::path::PathBuf {
            std::env::temp_dir().join(&self.bin_name)
        }

        fn bin_path_in_archive(&self) -> String {
            self.bin_name.clone()
        }

        fn show_download_progress(&self) -> bool {
            false
        }

        fn show_output(&self) -> bool {
            false
        }

        fn no_confirm(&self) -> bool {
            true
        }

        fn progress_template(&self) -> String {
            String::new()
        }

        fn progress_chars(&self) -> String {
            String::new()
        }

        fn auth_token(&self) -> Option<String> {
            None
        }

        fn update(&self) -> Result<Status> {
            self.update_calls.set(self.update_calls.get() + 1);
            if self.report_updated {
                Ok(Status::Updated("2.0.0".to_string()))
            } else {
                Ok(Status::UpToDate(self.current_version()))
            }
        }

        fn update_extended(&self) -> Result<UpdateStatus> {
            self.update_calls.set(self.update_calls.get() + 1);
            Ok(UpdateStatus::UpToDate)
        }
    }

    fn remove(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
    }

    // ---- RestartUpdate: guard behavior -------------------------------------
    //
    // These tests mutate a process-global environment variable, so they run
    // serially. The mock only reports `Updated` when the guard is set, which
    // means `inner.update` is short-circuited before any real re-exec occurs.

    #[test]
    #[serial_test::serial(env_guard)]
    fn update_is_skipped_when_guard_env_is_set() {
        let guard = "AUTO_UPDATE_TEST_GUARD_SET";
        unsafe { std::env::set_var(guard, "1") };

        let mock = MockRelease {
            report_updated: true,
            ..MockRelease::new("mock-guard-set")
        };
        let updater = RestartUpdate::new(mock).guard_env(guard);

        let status = updater.update().unwrap();

        assert!(matches!(status, Status::UpToDate(_)));
        assert_eq!(
            updater.inner.calls(),
            0,
            "inner update must be skipped when the guard is set"
        );

        unsafe { std::env::remove_var(guard) };
    }

    #[test]
    #[serial_test::serial(env_guard)]
    fn update_extended_is_skipped_when_guard_env_is_set() {
        let guard = "AUTO_UPDATE_TEST_GUARD_EXT";
        unsafe { std::env::set_var(guard, "1") };

        let updater = RestartUpdate::new(MockRelease::new("mock-guard-ext")).guard_env(guard);

        let status = updater.update_extended().unwrap();

        assert!(matches!(status, UpdateStatus::UpToDate));
        assert_eq!(updater.inner.calls(), 0);

        unsafe { std::env::remove_var(guard) };
    }

    #[test]
    #[serial_test::serial(env_guard)]
    fn update_runs_inner_when_guard_env_is_unset() {
        let guard = "AUTO_UPDATE_TEST_GUARD_UNSET";
        unsafe { std::env::remove_var(guard) };

        // Mock reports `UpToDate`, so no restart (process re-exec) is triggered.
        let updater = RestartUpdate::new(MockRelease::new("mock-guard-unset")).guard_env(guard);

        let status = updater.update().unwrap();

        assert!(matches!(status, Status::UpToDate(_)));
        assert_eq!(updater.inner.calls(), 1);
    }

    // ---- ThrottledUpdate: should_check / throttle file ---------------------
    //
    // Each test uses a distinct bin name, so their throttle files never
    // collide and the tests can run in parallel.

    #[test]
    fn should_check_is_true_without_a_throttle_file() {
        let updater = ThrottledUpdate::new(MockRelease::new("mock-throttle-missing"));
        remove(&updater.throttle_path());

        assert!(updater.should_check());
    }

    #[test]
    fn should_check_is_false_with_a_recent_throttle_file() {
        let updater = ThrottledUpdate::new(MockRelease::new("mock-throttle-recent"));
        let path = updater.throttle_path();
        remove(&path);

        updater.touch_throttle();

        assert!(!updater.should_check());
        remove(&path);
    }

    #[test]
    fn should_check_is_true_with_an_expired_throttle_file() {
        let updater = ThrottledUpdate::new(MockRelease::new("mock-throttle-expired"))
            .throttle_window(Duration::from_secs(60));
        let path = updater.throttle_path();
        std::fs::write(&path, "").unwrap();

        let expired = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            - Duration::from_secs(120);
        let expired = filetime::FileTime::from_unix_time(expired.as_secs() as i64, 0);
        filetime::set_file_mtime(&path, expired).unwrap();

        assert!(updater.should_check());
        remove(&path);
    }

    #[test]
    fn touch_throttle_creates_the_throttle_file() {
        let updater = ThrottledUpdate::new(MockRelease::new("mock-throttle-touch"));
        let path = updater.throttle_path();
        remove(&path);

        updater.touch_throttle();

        assert!(path.exists());
        remove(&path);
    }

    // ---- ThrottledUpdate: update / update_extended -------------------------

    #[test]
    fn update_runs_inner_and_touches_throttle_when_allowed() {
        let updater = ThrottledUpdate::new(MockRelease::new("mock-throttle-run"));
        let path = updater.throttle_path();
        remove(&path);

        let status = updater.update().unwrap();

        assert!(matches!(status, Status::UpToDate(_)));
        assert_eq!(updater.inner.calls(), 1);
        assert!(
            path.exists(),
            "throttle file should be touched after a check"
        );
        remove(&path);
    }

    #[test]
    fn update_skips_inner_when_throttled() {
        let updater = ThrottledUpdate::new(MockRelease::new("mock-throttle-skip"));
        let path = updater.throttle_path();
        remove(&path);
        updater.touch_throttle();

        let status = updater.update().unwrap();

        assert!(matches!(status, Status::UpToDate(_)));
        assert_eq!(updater.inner.calls(), 0);
        remove(&path);
    }

    #[test]
    fn update_extended_runs_inner_when_allowed() {
        let updater = ThrottledUpdate::new(MockRelease::new("mock-throttle-run-ext"));
        let path = updater.throttle_path();
        remove(&path);

        let status = updater.update_extended().unwrap();

        assert!(matches!(status, UpdateStatus::UpToDate));
        assert_eq!(updater.inner.calls(), 1);
        assert!(path.exists());
        remove(&path);
    }

    #[test]
    fn update_extended_skips_inner_when_throttled() {
        let updater = ThrottledUpdate::new(MockRelease::new("mock-throttle-skip-ext"));
        let path = updater.throttle_path();
        remove(&path);
        updater.touch_throttle();

        let status = updater.update_extended().unwrap();

        assert!(matches!(status, UpdateStatus::UpToDate));
        assert_eq!(updater.inner.calls(), 0);
        remove(&path);
    }
}
