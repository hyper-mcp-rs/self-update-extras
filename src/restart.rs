//! Restart wrapper: re-executes the process after a successful update.

use self_update::Status;
use self_update::errors::{Error, Result};
use self_update::update::{Release, ReleaseUpdate, UpdateStatus};

/// Default guard environment variable name.
const DEFAULT_GUARD_ENV: &str = "RESTART_GUARD";

/// Builder for a restart [`Update`].
///
/// Configure the inner [`ReleaseUpdate`] backend and an optional guard
/// environment variable, then call [`build`](Self::build) to produce a
/// `Box<dyn ReleaseUpdate>` that re-executes the process after a successful
/// update.
#[derive(Default)]
pub struct UpdateBuilder {
    release_update: Option<Box<dyn ReleaseUpdate>>,
    guard_env: Option<String>,
}

impl UpdateBuilder {
    /// Initialize a new builder.
    pub fn new() -> Self {
        Default::default()
    }

    /// Set the release update implementation to wrap. Required.
    pub fn release_update(&mut self, release_update: Box<dyn ReleaseUpdate>) -> &mut Self {
        self.release_update = Some(release_update);
        self
    }

    /// Set the guard environment variable name used to prevent restart loops.
    /// Defaults to `RESTART_GUARD`.
    pub fn guard_env(&mut self, env: &str) -> &mut Self {
        self.guard_env = Some(env.to_owned());
        self
    }

    /// Confirm config and create a ready-to-use `Update`.
    ///
    /// * Errors:
    ///     * Config - `release_update` was not provided
    pub fn build(&mut self) -> Result<Box<dyn ReleaseUpdate>> {
        let inner = self
            .release_update
            .take()
            .ok_or_else(|| Error::Config("`release_update` required".to_owned()))?;

        Ok(Box::new(Update {
            inner,
            guard_env: self
                .guard_env
                .clone()
                .unwrap_or_else(|| DEFAULT_GUARD_ENV.to_owned()),
        }))
    }
}

/// Wraps a [`ReleaseUpdate`] and restarts the process with the freshly
/// installed binary after a successful update.
///
/// After `update()` returns `Status::Updated`, the process is re-executed
/// using the new binary. A guard environment variable prevents restart loops:
/// once the restarted process detects the guard, it returns `UpToDate` instead
/// of restarting again.
///
/// # Platform behavior
///
/// - **Unix**: Replaces the current process image via `exec` (never returns on success).
/// - **Windows**: Spawns the new binary and exits with the child's status code.
///
/// # Guard environment variable
///
/// Defaults to `RESTART_GUARD` but can be customized via
/// [`UpdateBuilder::guard_env`](UpdateBuilder::guard_env). The value is set to
/// `"1"` during re-execution.
pub struct Update {
    inner: Box<dyn ReleaseUpdate>,
    guard_env: String,
}

impl Update {
    /// Initialize a new `Update` builder.
    pub fn configure() -> UpdateBuilder {
        UpdateBuilder::new()
    }

    /// Restart the process using the freshly installed executable.
    ///
    /// On Unix the current process image is replaced via `exec`, so this never
    /// returns on success. Windows has no `exec`, so the new binary is spawned
    /// and the current process exits with the child's status code.
    fn restart(&self) -> Result<()> {
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        let mut command = std::process::Command::new(self.inner.bin_install_path());
        command.args(&args).env(self.guard_env.clone(), "1");

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let err = command.exec();
            Err(Error::Release(format!(
                "re-executing updated binary: {}",
                err
            )))
        }

        #[cfg(windows)]
        {
            let status = command
                .status()
                .map_err(|e| Error::Release(format!("re-executing updated binary: {}", e)))?;
            std::process::exit(status.code().unwrap_or(0));
        }
    }
}

impl ReleaseUpdate for Update {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::MockRelease;

    // These tests mutate a process-global environment variable, so they run
    // serially. The mock only reports `Updated` when the guard is set, which
    // means `inner.update` is short-circuited before any real re-exec occurs.

    #[test]
    #[serial_test::serial(env_guard)]
    fn update_is_skipped_when_guard_env_is_set() {
        let guard = "AUTO_UPDATE_TEST_GUARD_SET";
        unsafe { std::env::set_var(guard, "1") };

        let mock = MockRelease::new("mock-guard-set").report_updated(true);
        let calls = mock.call_counter();
        let updater = Update::configure()
            .release_update(Box::new(mock))
            .guard_env(guard)
            .build()
            .unwrap();

        let status = updater.update().unwrap();

        assert!(matches!(status, Status::UpToDate(_)));
        assert_eq!(
            calls.get(),
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

        let mock = MockRelease::new("mock-guard-ext");
        let calls = mock.call_counter();
        let updater = Update::configure()
            .release_update(Box::new(mock))
            .guard_env(guard)
            .build()
            .unwrap();

        let status = updater.update_extended().unwrap();

        assert!(matches!(status, UpdateStatus::UpToDate));
        assert_eq!(calls.get(), 0);

        unsafe { std::env::remove_var(guard) };
    }

    #[test]
    #[serial_test::serial(env_guard)]
    fn update_runs_inner_when_guard_env_is_unset() {
        let guard = "AUTO_UPDATE_TEST_GUARD_UNSET";
        unsafe { std::env::remove_var(guard) };

        // Mock reports `UpToDate`, so no restart (process re-exec) is triggered.
        let mock = MockRelease::new("mock-guard-unset");
        let calls = mock.call_counter();
        let updater = Update::configure()
            .release_update(Box::new(mock))
            .guard_env(guard)
            .build()
            .unwrap();

        let status = updater.update().unwrap();

        assert!(matches!(status, Status::UpToDate(_)));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn build_requires_a_release_update() {
        assert!(Update::configure().build().is_err());
    }

    #[test]
    fn metadata_methods_delegate_to_inner() {
        let updater = Update::configure()
            .release_update(Box::new(MockRelease::new("mock-forward")))
            .build()
            .unwrap();

        assert_eq!(updater.current_version(), "1.0.0");
        assert_eq!(updater.target(), "test-target");
        assert_eq!(updater.target_version(), None);
        assert_eq!(updater.bin_name(), "mock-forward");
        assert_eq!(
            updater.bin_install_path(),
            std::env::temp_dir().join("mock-forward")
        );
        assert_eq!(updater.bin_path_in_archive(), "mock-forward");
        assert!(!updater.show_download_progress());
        assert!(!updater.show_output());
        assert!(updater.no_confirm());
        assert_eq!(updater.progress_template(), "");
        assert_eq!(updater.progress_chars(), "");
        assert_eq!(updater.auth_token(), None);
        assert!(updater.get_latest_release().is_ok());
        assert!(updater.get_latest_releases("1.0.0").is_ok());
        assert!(updater.get_release_version("1.0.0").is_ok());
    }
}
