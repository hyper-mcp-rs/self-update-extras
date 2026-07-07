# auto-update

A reusable Rust library for self-updating CLI binaries. It checks GitHub releases for newer versions, downloads and verifies release assets, replaces the running executable, and re-executes with the original arguments.

## Purpose

This crate provides auto-update functionality for `hyper-mcp-*` binaries as an internal utility. It handles:

- **Release checking**: Queries GitHub releases to find versions newer than the current binary
- **Checksum verification**: Downloads and verifies release assets against published checksums (integrity check against accidental corruption)
- **Binary replacement**: Atomically replaces the running executable
- **Process restart**: Re-executes with original arguments using Unix `exec()` semantics
- **Throttling**: Limits update checks to a configurable time window (default: 15 minutes)
- **Restart guard**: Prevents update loops via environment variable

**Note on Windows**: Auto-update is currently Unix-focused. Windows support can be enabled but re-exec semantics differ; see `WindowsPolicy`.

## Usage as a Submodule

This crate is intended to be included as a Git submodule in hyper-mcp projects:

```bash
# Add as submodule in your project root
git submodule add https://github.com/hyper-mcp-rs/auto-update.git auto-update

# Commit the submodule
git commit -m "Add auto-update submodule"
```

### Integration Example

```rust
use auto_update::{Updater, WindowsPolicy};

#[tokio::main]
async fn main() {
    let updater = Updater::new()
        .repo_owner("hyper-mcp-rs")
        .repo_name("hyper-mcp")  // Your GitHub repo owner
        .binary_name("hyper-mcp")   // Binary name in releases
        .guard_env("HYPER_MCP_AUTO_UPDATED")  // Prevent restart loops
        .throttle_file("hyper-mcp-update-check")  // Throttle state file
        .windows_policy(WindowsPolicy::Disabled);  // Or Enabled if supported

    if let Err(e) = updater.run().await {
        tracing::warn!(error = ?e, "Auto-update failed; continuing with the current version");
    }

    // Rest of your application...
}
```

### Configuration

All fields are customizable via builder-style methods:

| Method | Description |
|--------|-------------|
| `repo_owner(owner)` | GitHub owner/org (e.g., `"hyper-mcp-rs"`) |
| `repo_name(name)` | Repository name (e.g., `"hyper-mcp"`) |
| `binary_name(name)` | Binary name in release assets |
| `guard_env(env)` | Environment variable to prevent restart loops |
| `throttle_file(name)` | Throttle state file name (stored in `$TMPDIR`) |
| `throttle_window(duration)` | Minimum interval between checks |
| `windows_policy(policy)` | Enable/disable auto-update on Windows |

### Build Requirements

The crate requires the `BUILD_TARGET` environment variable to be set at build time (automatically provided by Cargo as `TARGET`). This ensures the correct target-specific asset is downloaded.

## License

Apache-2.0 — see [LICENSE](./LICENSE) for details.
