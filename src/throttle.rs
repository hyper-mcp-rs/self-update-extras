//! Throttle wrapper: limits how often update checks run.

use self_update::Status;
use self_update::errors::{Error, Result};
use self_update::update::{Release, ReleaseUpdate, UpdateStatus};
use std::time::Duration;

/// Default minimum time between update checks.
const DEFAULT_THROTTLE_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Builder for a throttled [`Update`].
///
/// Configure the inner [`ReleaseUpdate`] backend and the throttle window,
/// then call [`build`](Self::build) to produce a `Box<dyn ReleaseUpdate>`
/// that skips update checks within the configured window.
#[derive(Default)]
pub struct UpdateBuilder {
    release_update: Option<Box<dyn ReleaseUpdate>>,
    throttle_window: Option<Duration>,
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

    /// Set the minimum time between update checks. Defaults to 15 minutes.
    pub fn throttle_window(&mut self, window: Duration) -> &mut Self {
        self.throttle_window = Some(window);
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
            throttle_window: self.throttle_window.unwrap_or(DEFAULT_THROTTLE_WINDOW),
        }))
    }
}

/// Wraps a [`ReleaseUpdate`] and throttles how often update checks run.
///
/// The underlying update check is skipped when one was already performed
/// within `throttle_window`. The time of the last check is tracked via a
/// throttle file in the system temp directory.
///
/// # Throttle behavior
///
/// The throttle file is touched only after a *successful* update check.
/// Failed checks do not reset the throttle window, allowing retries without
/// waiting for the full window to elapse.
pub struct Update {
    inner: Box<dyn ReleaseUpdate>,
    throttle_window: Duration,
}

impl Update {
    /// Initialize a new `Update` builder.
    pub fn configure() -> UpdateBuilder {
        UpdateBuilder::new()
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
    use crate::test_support::MockRelease;

    fn remove(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
    }

    /// Build a concrete `Update` for white-box tests of the private throttle
    /// mechanics. `build()` returns a `Box<dyn ReleaseUpdate>`, which hides
    /// those methods, so tests construct the type directly.
    fn concrete(mock: MockRelease, window: Duration) -> Update {
        Update {
            inner: Box::new(mock),
            throttle_window: window,
        }
    }

    // Each test uses a distinct bin name, so their throttle files never
    // collide and the tests can run in parallel.

    #[test]
    fn should_check_is_true_without_a_throttle_file() {
        let updater = concrete(
            MockRelease::new("mock-throttle-missing"),
            DEFAULT_THROTTLE_WINDOW,
        );
        remove(&updater.throttle_path());

        assert!(updater.should_check());
    }

    #[test]
    fn should_check_is_false_with_a_recent_throttle_file() {
        let updater = concrete(
            MockRelease::new("mock-throttle-recent"),
            DEFAULT_THROTTLE_WINDOW,
        );
        let path = updater.throttle_path();
        remove(&path);

        updater.touch_throttle();

        assert!(!updater.should_check());
        remove(&path);
    }

    #[test]
    fn should_check_is_true_with_an_expired_throttle_file() {
        let updater = concrete(
            MockRelease::new("mock-throttle-expired"),
            Duration::from_secs(60),
        );
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
        let updater = concrete(
            MockRelease::new("mock-throttle-touch"),
            DEFAULT_THROTTLE_WINDOW,
        );
        let path = updater.throttle_path();
        remove(&path);

        updater.touch_throttle();

        assert!(path.exists());
        remove(&path);
    }

    #[test]
    fn update_runs_inner_and_touches_throttle_when_allowed() {
        let mock = MockRelease::new("mock-throttle-run");
        let calls = mock.call_counter();
        let updater = concrete(mock, DEFAULT_THROTTLE_WINDOW);
        let path = updater.throttle_path();
        remove(&path);

        let status = updater.update().unwrap();

        assert!(matches!(status, Status::UpToDate(_)));
        assert_eq!(calls.get(), 1);
        assert!(
            path.exists(),
            "throttle file should be touched after a check"
        );
        remove(&path);
    }

    #[test]
    fn update_skips_inner_when_throttled() {
        let mock = MockRelease::new("mock-throttle-skip");
        let calls = mock.call_counter();
        let updater = concrete(mock, DEFAULT_THROTTLE_WINDOW);
        let path = updater.throttle_path();
        remove(&path);
        updater.touch_throttle();

        let status = updater.update().unwrap();

        assert!(matches!(status, Status::UpToDate(_)));
        assert_eq!(calls.get(), 0);
        remove(&path);
    }

    #[test]
    fn update_extended_runs_inner_when_allowed() {
        let mock = MockRelease::new("mock-throttle-run-ext");
        let calls = mock.call_counter();
        let updater = concrete(mock, DEFAULT_THROTTLE_WINDOW);
        let path = updater.throttle_path();
        remove(&path);

        let status = updater.update_extended().unwrap();

        assert!(matches!(status, UpdateStatus::UpToDate));
        assert_eq!(calls.get(), 1);
        assert!(path.exists());
        remove(&path);
    }

    #[test]
    fn update_extended_skips_inner_when_throttled() {
        let mock = MockRelease::new("mock-throttle-skip-ext");
        let calls = mock.call_counter();
        let updater = concrete(mock, DEFAULT_THROTTLE_WINDOW);
        let path = updater.throttle_path();
        remove(&path);
        updater.touch_throttle();

        let status = updater.update_extended().unwrap();

        assert!(matches!(status, UpdateStatus::UpToDate));
        assert_eq!(calls.get(), 0);
        remove(&path);
    }

    #[test]
    fn build_requires_a_release_update() {
        assert!(Update::configure().build().is_err());
    }

    #[test]
    fn builder_wraps_the_release_update() {
        let mock = MockRelease::new("mock-throttle-builder");
        let calls = mock.call_counter();
        let path = std::env::temp_dir().join("mock-throttle-builder.throttle");
        remove(&path);

        let updater = Update::configure()
            .release_update(Box::new(mock))
            .throttle_window(Duration::from_secs(60))
            .build()
            .unwrap();

        updater.update().unwrap();

        assert_eq!(calls.get(), 1);
        remove(&path);
    }
}
