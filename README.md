# .NET Debugger for Zed

Debug C#, F# and VB.NET in Zed with [netcoredbg](https://github.com/Samsung/netcoredbg) (open source, DAP over stdio). Does not use Microsoft's `vsdbg`, whose licence restricts it to Microsoft products.

> **Status: the code builds, and the unit tests pass in CI.** Every push and pull request runs `cargo test --workspace` plus a release build for `wasm32-wasip2`, the target Zed loads. A full interactive Zed debug session (Install Dev Extension, then launch/attach and F5 on a `dotnet run` task) still needs a manual check before marketplace release.

## What it does

- **Verified install.** netcoredbg `3.2.0-1092` is downloaded once, checked against a SHA-256 baked into the extension (not fetched from the network), extracted with path-traversal/zip-bomb guards, and published by atomic rename. After that, launching a session does a single `stat`: no network, no hashing.
- **Binary resolution order:** `dap.netcoredbg.binary` in Zed settings → `netcoredbg` on `PATH` → the pinned download.
- **Launch and attach** with validated config and clear error messages (`program`, `args`, `cwd`, `env`, `stopAtEntry`, `justMyCode`, `enableStepFiltering`, `processId`). A JSON schema gives completion and validation in `debug.json`.
- **F5 on `dotnet run` tasks.** The task becomes `dotnet build` and then MSBuild is asked for the real output path (`-getProperty:TargetPath`), so multi-targeting, custom output paths and configurations just work. `ASPNETCORE_ENVIRONMENT` / `DOTNET_ENVIRONMENT` default to `Development`.
- **Prebuilt platforms:** Linux x64/arm64, macOS arm64, Windows x64. Intel Macs and Windows arm64 have no upstream build; set `dap.netcoredbg.binary` instead.

Performance note: the extension only runs at session start. Stepping and evaluation speed are netcoredbg's; the extension keeps startup minimal (cached path, size-optimised LTO wasm).

## Example `.zed/debug.json`

```json
[
  {
    "label": "API",
    "adapter": "netcoredbg",
    "request": "launch",
    "build": { "command": "dotnet", "args": ["build"] },
    "program": "$ZED_WORKTREE_ROOT/bin/Debug/net10.0/App.dll",
    "cwd": "$ZED_WORKTREE_ROOT",
    "env": { "ASPNETCORE_ENVIRONMENT": "Development" }
  },
  { "label": "Attach", "adapter": "netcoredbg", "request": "attach", "processId": "${ZED_PICK_PID}" }
]
```

(`${ZED_PICK_PID}` is a guess at Zed's process-picker variable; check the current Zed docs.)

## Verify

```sh
rustup target add wasm32-wasip2
cargo test -p dotnet-debugger-core         # host; needs a C toolchain
cargo build --release --target wasm32-wasip2
```

Then in Zed: `zed: install dev extension` and select this directory. Core tests also run without a C toolchain: `cargo test -p dotnet-debugger-core --target wasm32-wasip1` with `CARGO_TARGET_WASM32_WASIP1_RUNNER="node scripts/run-wasi-tests.mjs"`.

## Known limits

- `dotnet test` debugging is not implemented (needs the VSTest attach handshake).
- No `launchSettings.json` support; set `env` explicitly.
- Expression evaluation is netcoredbg's: LINQ and some lambdas fail.
- Not exercised against a real Zed build; the exact moment Zed calls `dap_config_to_scenario` after a locator is assumed from the API docs.

## Dependencies

| Package | Source | Purpose |
|---|---|---|
| zed_extension_api 0.7.0 | crates.io, Zed Industries | host bindings |
| serde_json 1 | crates.io, serde-rs | JSON |
| sha2 0.10 | crates.io, RustCrypto | archive checksum |
| flate2 1 (miniz_oxide) | crates.io, rust-lang | gzip |
| tar 0.4 | crates.io, alexcrichton | tar |
| zip 9.5 | crates.io, zip-rs | zip |
| netcoredbg 3.2.0-1092 | github.com/Samsung/netcoredbg (MIT) | the debugger; archive SHA-256s match GitHub's published digests |
