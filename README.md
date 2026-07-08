# self-update-extras

Self-updating support for CLI binaries.

`self-update-extras` provides two small, composable wrappers around any type that
implements [`self_update`](https://crates.io/crates/self_update)'s
`ReleaseUpdate` trait. Each wrapper is itself a `ReleaseUpdate`, so they layer
over a backend — or over each other — and can be used anywhere a
`ReleaseUpdate` is expected.

- **`throttle::Update`** — limits how often update checks run, recording the
  time of the last check in a throttle file in the system temp directory.
- **`restart::Update`** — re-executes the process with the freshly installed
  binary after a successful update, using a guard environment variable to
  prevent restart loops.

The actual update source (GitHub, a custom server, etc.) is supplied by the
caller as any `ReleaseUpdate` implementation, e.g. one of `self_update`'s
backends.

## Installation

```toml
[dependencies]
self-update-extras = "0.1"
self_update = "0.44"
```

## Usage

Each wrapper follows `self_update`'s builder convention: `Update::configure()`
returns an `UpdateBuilder`, and `build()` produces a
`Box<dyn ReleaseUpdate>`.

```rust
use self_update_extras::{restart, throttle};
use self_update::backends::github;
use self_update::update::ReleaseUpdate;
use std::time::Duration;

fn check_for_update() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Any `ReleaseUpdate` implementation — here, self_update's GitHub backend.
    let backend = github::Update::configure()
        .repo_owner("my-org")
        .repo_name("my-app")
        .bin_name("my-app")
        .current_version(self_update::cargo_crate_version!())
        .no_confirm(true)
        .build()?;

    // 2. Throttle how often the check actually runs.
    let throttled = throttle::Update::configure()
        .release_update(backend)
        .throttle_window(Duration::from_secs(15 * 60))
        .build()?;

    // 3. Restart into the new binary after a successful update.
    //    `restart` must be the OUTERMOST wrapper — see the note below.
    let updater = restart::Update::configure()
        .release_update(throttled)
        .guard_env("MY_APP_AUTO_UPDATED")
        .build()?;

    // Runs the check, respecting the throttle window and restart guard.
    let status = updater.update()?;
    println!("update status: {status:?}");
    Ok(())
}
```

### Composition order matters

Wrap from the inside out as **backend → throttle → restart**, so that
`restart` is the outermost layer.

On a successful update, `restart` replaces the current process (`exec` on
Unix) or spawns the new binary and exits (Windows) — in both cases the call
**never returns on success**. If `throttle` were the outer wrapper, its
"record the check time" step would never run because the process would already
have been replaced.

## Wrappers

### `throttle::Update`

| Builder method | Description |
|----------------|-------------|
| `release_update(Box<dyn ReleaseUpdate>)` | The wrapped updater. **Required.** |
| `throttle_window(Duration)` | Minimum interval between checks. Default: 15 minutes. |
| `build()` | Returns `Result<Box<dyn ReleaseUpdate>>`; errors if `release_update` is missing. |

When `update()` is called, the check is skipped (returning `UpToDate`) if the
throttle file was modified within `throttle_window`. Otherwise the wrapped
updater runs and the throttle file is touched. The file lives at
`<temp_dir>/<bin_name>.throttle`, where `bin_name` comes from the wrapped
updater.

### `restart::Update`

| Builder method | Description |
|----------------|-------------|
| `release_update(Box<dyn ReleaseUpdate>)` | The wrapped updater. **Required.** |
| `guard_env(&str)` | Guard environment variable used to prevent restart loops. Default: `RESTART_GUARD`. |
| `build()` | Returns `Result<Box<dyn ReleaseUpdate>>`; errors if `release_update` is missing. |

When `update()` returns `Updated`, the process restarts into the freshly
installed binary, forwarding the original arguments and setting the guard
variable. On the re-executed run the guard is detected and the check is
skipped, so the update happens at most once per launch. Restart is supported
on both Unix (via `exec`) and Windows (spawn-and-exit, propagating the child's
exit code).

## License

Apache-2.0 — see [LICENSE](./LICENSE) for details.
