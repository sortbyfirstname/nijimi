<!--suppress HtmlDeprecatedAttribute -->
<p align="center">
  <img src="assets/nijimi-text-logo.svg" alt="nijimi" width="262">
</p>

<p align="center">
  <strong>Ambient lighting for the black bars around your games.</strong><br>
  Lightweight, automatic, and out of your way.
</p>

<p align="center">
  <em><strong>nijimi</strong> (滲み): Japanese for the soft bleeding of colour at an edge, like ink spreading into paper.</em>
</p>

---

Playing a 16:9 game on an ultrawide monitor, or a 4:3 classic on a widescreen one, leaves you with large black bars on either side. nijimi fills those bars with a soft, blurred glow taken from the edges of the game itself, so the picture seems to spill out across the whole screen instead of stopping at a hard edge.

It's built to be forgotten about. Set an aspect ratio for each game once, and nijimi switches on by itself when that game is in front and off again when you Alt-Tab away. There's no window to manage, nothing to install, and no background service.

It's also built to cost your games next to nothing. The whole effect runs on the GPU at a fraction of your screen's resolution, nothing renders unless a configured game is in focus, and when idle it uses effectively no CPU (see [the numbers](#lightweight-by-design)).

## Features

- **Per-game profiles.** Give each game its own aspect ratio (`16:9`, `21:9`, `32:9`, `16:10`, `4:3`, or any custom `width:height`).
- **Automatic.** Turns on when a configured game is the foreground app on your primary display, and pauses on Alt-Tab.
- **Lightweight.** Capture, blur, and compositing all stay on the GPU, and frames are never copied back to the CPU. Nothing renders while you're not playing, and there's no polling: it reacts to Windows events.
- **Never covers the game.** The centred game area is left untouched; the effect is drawn only outside it.
- **Click-through.** The overlay never takes focus or intercepts input.
- **Portable.** A single executable, with settings saved in an INI file next to it. No installer, no service. Optionally starts with Windows.

## Lightweight by design

nijimi exists to make games look better, so it can't be allowed to make them run worse. Every design decision favours staying off the game's critical path:

- **Zero work when idle.** No game in focus means no capture, no rendering, and no timers. The app sleeps until Windows reports a foreground change.
- **Nothing held when idle.** The graphics device, overlay surfaces, and capture are created only when a configured game comes to the front, and fully released a minute after it leaves (so a quick Alt-Tab and back is instant). Idle, nijimi holds no GPU memory at all.
- **GPU-only pipeline.** Desktop frames are captured, blurred, and composited entirely in GPU memory. No pixels ever cross to the CPU.
- **Cheap by construction.** The blur is computed at quarter resolution with separable passes, refreshed about 30 times a second rather than at your monitor's refresh rate, and only drawn outside the game area.
- **Doesn't touch the game.** No injection, no hooks inside the game's process, no driver. nijimi is a separate click-through window and nothing is ever loaded into the game.
- **UI only when you open it.** The Settings window runs as its own short-lived process and exits when closed, so the always-running part carries no UI framework.

### Measured

Release build on a Ryzen 7 9800X3D with an RTX 5090 at 5120×2160, 165 Hz:

| | Idle in the tray | Idle after a game session |
|---|---|---|
| CPU | **0 ms** over 60 seconds | **0 ms** |
| GPU usage | **0%** (no graphics device) | **0%** (no graphics device) |
| GPU memory | **0 MB** | **0 MB** |
| Memory | **1.6 MB** | **≈4 MB** |
| Threads | 1 | 6 |

Memory is the private working set, the same figure Task Manager shows in its Memory column. It's slightly higher after a game session because Windows keeps the graphics driver loaded once it's been used. The total working set, which also counts Windows and graphics-driver code that other processes can share, is about 12 MB idle in the tray and about 21 MB after a game session.

Alt-Tab back into a game within a minute and the overlay is back in about **11 ms**. After longer than that, it's ready in about **110 ms**, most of which is the graphics driver setting up a device. Either way, this happens on nijimi's own thread while the game keeps running normally.

While a game is running, measured over 60 seconds of gameplay in Persona 5 Royal with the effect active:

| nijimi while active | Average | 95th percentile |
|---|---|---|
| CPU | **0.08%** of total CPU | 0.44% |
| GPU (3D engine) | **0.51%** | 0.55% |
| GPU memory | 287 MB | 287 MB |
| Memory | 17 MB | 17 MB |

For scale, the game itself used 8.5% of the GPU over the same period. After Alt-Tabbing out, nijimi's GPU use dropped to 0% immediately and its GPU memory to 202 MB, then to 0 MB after a minute. GPU cost scales with display resolution and GPU speed, so expect a proportionally higher percentage on slower cards or a lower one at lower resolutions. To measure on your own system, run `scripts/measure-performance.ps1` while playing.

## Getting started

1. Download the latest `nijimi-…-windows-x64.zip` from [Releases](../../releases/latest) (or [build it yourself](#building)) and extract it to a folder you can write to.
2. Run it. The icon appears in the system tray.
3. Click the tray icon to open **Settings**, choose **Add app…**, and pick your game from recently used apps, running processes, or **Browse…**.
4. Pick an aspect ratio. Changes save automatically.
5. Launch the game. The ambient effect appears in the bars as soon as it's in front.

Right-click the tray icon for **Settings** and **Exit**.

### Requirements

- Windows 10 version 2004 or later
- A Direct3D 11-capable GPU

Nothing else is needed. `nijimi.exe` is fully self-contained and doesn't need the Visual C++ Redistributable or any other runtime.

## Settings

- **Program settings.** Lists your game profiles. Search by executable name or path, select a profile to change its aspect ratio, or remove it. Supported ratios range from `1:4` to `4:1`; invalid entries are rejected and the previous value is restored.
- **Global settings.** Sets the default aspect ratio for new profiles (initially `21:9`); existing profiles keep their own ratios. **Always on** shows the effect whatever app is in front: profiled games still use their own ratio, and everything else uses the default. It's off by default, and while it's on nijimi keeps rendering instead of going idle. **Show in screenshots** lets screenshots and recordings capture the effect; it's off by default, so they normally show plain black bars. **Start with Windows** launches nijimi in the tray when you sign in. It's off by default and applies to the copy you turn it on from. If you move the executable, turn it off and on again.
- **Add app.** Shows apps that were in the foreground while nijimi was running, followed by currently running processes (newest first). The list refreshes when you open it and with the refresh button. Windows may block access to elevated or protected processes.

Settings are stored in `nijimi.ini` beside the executable. To move your setup, copy the executable and the INI file together. To carry settings over from an older build, copy its INI file next to the new executable and rename it to `nijimi.ini`.

If the executable's folder isn't writable (for example, if you put it in `Program Files`), settings are stored in `%APPDATA%\nijimi\nijimi.ini` instead. A settings file beside the executable always takes priority.

Start with Windows uses your per-user startup list (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`), so it doesn't need admin rights. It also shows up in Task Manager's **Startup apps**, where it can be turned off too.

## Limitations

- **Primary display only.** The effect is applied to the primary display only.
- **Fixed per-game ratio.** The ratio comes from the profile; nijimi doesn't detect a game's actual resolution, or content inside an ordinary window.
- **Some content can't be captured.** Protected video and some exclusive-fullscreen modes may not be capturable.
- **No HDR-specific processing.**
- **Burn-in.** Ambient lighting may reduce the contrast of static bars, but it can't guarantee to prevent or reverse display burn-in.
- **Screenshots.** By default the overlay is excluded from screen capture, so screenshots and recordings don't show it even though it's visible on your monitor. Turn on **Show in screenshots** in Global settings to include it.

---

## Technical details

### Architecture

nijimi is written in Rust and runs as two parts of the same executable.

- **Tray process (always running).** A small Win32 message loop that owns the tray icon, watches for foreground-window changes, and drives the Direct3D 11 renderer. Foreground tracking is event-driven via `SetWinEventHook`, with no polling.
- **Settings process (on demand).** Launched as a short-lived child process (`nijimi.exe --settings`) with a [GPUI](https://www.gpui.rs/) interface. GPUI is only initialised there, so the always-running process never pays for UI memory or rendering. Changes are sent back to the tray process as soon as they're saved.

Only one tray instance runs per Windows session. If Windows Explorer restarts, the tray icon is restored.

### Rendering pipeline

1. **Capture.** The desktop is captured with DXGI Desktop Duplication. Frames stay in GPU memory and are never copied to the CPU. The latest frame is kept in a texture and redrawn even when DXGI reports no new frame.
2. **Sample.** The content near the edges of the protected rectangle is reflected outward and filtered at quarter resolution.
3. **Blur.** Wider separable blur passes produce a broad, soft glow, followed by a distance fade away from the content.
4. **Composite.** The full-resolution result is clipped strictly to the area *outside* the protected rectangle and presented through a click-through DirectComposition overlay. The overlay excludes itself from capture so it never feeds back into its own input.

The protected rectangle is computed with integer geometry, fitted and centred on the primary display for the selected ratio.

### Performance

The project's main goal is minimal impact on the game you're playing.

- **Idle.** When the configured game leaves the foreground, rendering and capture stop immediately and the overlay is hidden. The Direct3D device, swap chain, composition objects, and textures are kept for one minute so a quick return is instant, then all released. Only the hidden overlay window remains.
- **Precompiled shaders.** `build.rs` compiles `src/ambient.hlsl` to bytecode at build time, so the HLSL compiler is never loaded and no shaders are compiled at runtime.
- **Active.** The render loop runs at about 30 fps, entirely on the GPU, and blurs at reduced resolution.
- **Recovery.** If capture access is lost (for example on a UAC prompt or a mode switch), the overlay is cleared and capture is retried with a backoff of up to four seconds. Resources are rebuilt after display changes. If rendering fails, it stops rather than retrying on every window switch, and tries again when settings are saved or the display configuration changes.
- **Process lookup.** If Windows refuses access to a process's path, it's treated as not matching any profile, so the effect stays off.

Gameplay frame-time impact has not yet been formally measured.

### Building

Install the stable Rust toolchain, then:

```powershell
cargo build --release            # optimised build for distribution (full LTO)
cargo run --profile dev-release  # faster-building optimised profile for day-to-day testing
cargo test                       # geometry, settings, activation rules, capture retry, WARP blur tests
```

The release profile uses full LTO with a single codegen unit, so the final link step takes a minute or two. The `dev-release` profile uses thin LTO for much faster rebuilds and is intended for testing only. Ship builds from `--release`.

In RustRover, the shared **Run** configuration launches the app with `dev-release`, and **Build** produces the full release executable.

The app icon is embedded as a Windows resource from `assets/tresbient.ico`, and the UI's SVG assets are compiled into the binary. Windows may keep showing a cached icon until Explorer refreshes its icon cache.

### Releasing

The **Build** GitHub Actions workflow tests every push and pull request. To publish a release, bump `version` in `Cargo.toml` and push to `main`. If that version has no release yet, the workflow builds it, creates the `v<version>` tag, and publishes a GitHub release with `nijimi-v<version>-windows-x64.zip` (exe, README, and license) and its SHA-256 checksum. Pushes that don't change the version only run the tests.

### Tests

`cargo test` covers aspect-ratio geometry (including portrait and odd-sized displays), settings round trips and activation rules, capture retry behaviour, and offscreen Direct3D WARP rendering of the blur passes.

## License

[MIT](LICENSE)
