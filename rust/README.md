# ComfyUI Rust Migration

This directory is the root of an incremental migration of ComfyUI to Rust
with a native WinUI 3 desktop shell. It is a Cargo workspace that lives
alongside the Python code; nothing in the existing Python application is
modified. The Python backend remains the source of truth while pieces are
ported crate by crate.

## Crates

| Crate | Purpose | Status |
| ----- | ------- | ------ |
| `comfy-graph` | Port of `comfy_execution/graph.py` and `graph_utils.py`: prompt graph model, `GraphBuilder`, `DynamicPrompt`, topological sort, dependency-cycle detection. Graphs serialize to the exact JSON shape of the ComfyUI API prompt format, so output is interchangeable with the Python backend. | Ported, unit tested |
| `comfy-shell` | WinUI 3 desktop shell built with windows-rs. Hosts Microsoft.UI.Xaml content in a Win32 window through `DesktopWindowXamlSource` (WinUI 3 islands) and ships as an AppContainer MSIX. | Working scaffold |

## Building

Prerequisites: Rust 1.95+, Windows 11, Windows SDK (for `makeappx`),
and the Windows App Runtime 2.x installed (for unpackaged development runs).

```powershell
# One-time: download Windows App SDK metadata + bootstrap DLL from NuGet
.\tools\fetch-winappsdk.ps1

# Core crates (no Windows App SDK needed)
cargo build
cargo test

# WinUI 3 shell (generates windows-rs bindings from the fetched metadata)
cargo build -p comfy-shell
cargo run -p comfy-shell

# AppContainer MSIX package (unsigned; see script header for dev signing)
.\tools\package-msix.ps1
```

## Architecture notes

### WinUI 3 from Rust

windows-rs cannot yet implement composable WinRT classes, which rules out
subclassing `Microsoft.UI.Xaml.Application` (the way C#/C++ WinUI apps
start). Instead the shell uses the supported islands path, which needs no
composition:

1. `DispatcherQueueController::CreateOnCurrentThread`
2. `WindowsXamlManager::InitializeForCurrentThread`
3. A plain Win32 window (windows-rs)
4. `DesktopWindowXamlSource` attached to the window's `WindowId`,
   `SiteBridge().MoveAndResize(...)` on `WM_SIZE`

Bindings for `Microsoft.UI.*` are generated at build time by
`windows-bindgen` from the Windows App SDK 2.2 winmd files (fetched from
NuGet by `tools/fetch-winappsdk.ps1`; nothing vendored). Types are mapped
onto the released `windows`, `windows-collections`, `windows-numerics`, and
`windows-future` crates through explicit `--reference` entries in
`crates/comfy-shell/build.rs`. The generator's built-in reference defaults
assume unreleased versions of the split crates, so the build script pins
`windows-bindgen 0.64` and declares references type by type where the
released crates and the generator disagree.

When windows-rs gains composition support, the shell can move from an
island in a Win32 window to a full `Microsoft.UI.Xaml.Application` +
`Microsoft.UI.Xaml.Window`.

### AppContainer packaging

`crates/comfy-shell/packaging/AppxManifest.xml` declares
`uap10:TrustLevel="appContainer"` with `uap10:RuntimeBehavior="packagedClassicApp"`,
so the shell runs in an AppContainer (process isolation, brokered resource
access, no runFullTrust). WinUI 3 comes from the
`Microsoft.WindowsAppRuntime.2` framework package dependency inside the
container. `tools/package-msix.ps1` builds and packs the MSIX.

AppContainer processes cannot reach localhost by default. While the Python
backend still runs out of process, development machines need a loopback
exemption:

```powershell
CheckNetIsolation.exe LoopbackExempt -a -n=(Get-AppxPackage ComfyUI.Shell).PackageFamilyName
```

### Migration plan

The port proceeds bottom-up, keeping the Python backend runnable at every
step:

1. **comfy-graph** (done): pure graph/scheduling logic, no torch coupling.
2. **Prompt validation + API types**: port `execution.py` validation and the
   `/prompt` API surface into a `comfy-api` crate; verify against the Python
   implementation with shared JSON fixtures.
3. **Shell grows real UI**: queue view, progress events over the existing
   websocket protocol, then embed the frontend via the XAML `WebView2`
   control (its Core metadata is already part of the binding set) while
   native panels replace it screen by screen.
4. **Execution engine**: `comfy-exec` crate driving the sampling loop
   through native inference backends (candle/burn/onnxruntime evaluation),
   falling back to the Python worker per node family until parity.
