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

**Without nijimi**, a 16:9 game on a 21:9 monitor:

![Persona 5 Royal's intro in 16:9 on an ultrawide monitor, with black bars on both sides](assets/screenshots/p5r-intro-before.png)

**With nijimi**, the bars glow with the colours at the edges of the picture:

![The same frame with nijimi on: the green and yellow of the picture's edges blur softly out into the bars](assets/screenshots/p5r-intro-after.png)

It's built to be forgotten about. Set an aspect ratio for each game once, and nijimi switches on by itself when that game is in front and off again when you Alt-Tab away. There's no window to manage, nothing to install, and no background service.

It's also built to cost your games next to nothing. The whole effect runs on the GPU, with the glow blurred at a fraction of your screen's resolution, nothing renders unless a configured game is in focus, and when idle it uses effectively no CPU (see [the numbers](#lightweight-by-design)).

## Features

- **Per-game profiles.** Give each game its own aspect ratio (`16:9`, `21:9`, `32:9`, `16:10`, `4:3`, or any custom `width:height`).
- **Tunable look.** Set edge softness (soften the picture's outermost few pixels), edge blend (blur the picture gradually into the bars instead of a hard line), blur strength, and blur resolution (full down to an eighth) globally, and override them for any game. A live preview in Settings shows the result on a sample image, sized to your monitor.
- **Frame rate.** Update the effect 30 times a second (default) or 60 for smoother motion.
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
- **Cheap by construction.** The glow is blurred at reduced resolution (a quarter by default, never finer than half) with separable passes, and only in and around the bars rather than across the whole screen. It's refreshed 30 times a second by default rather than at your monitor's refresh rate, and redrawn only when the screen has actually changed.
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

GPU time to draw one frame of the effect, at 5120×2160 with 16:9 content and the default look (median of 240 frames, measured with GPU timestamps):

| Blur resolution | GPU time per frame | At 30 fps | At 60 fps |
|---|---|---|---|
| Eighth | 0.057 ms | 0.17% | 0.34% |
| Quarter (default) | 0.070 ms | 0.21% | 0.42% |
| Half | 0.164 ms | 0.49% | 0.98% |
| Full | 0.205 ms | 0.62% | 1.2% |

The percentages are the share of one second of GPU time; capturing the screen and Windows compositing the overlay add a little on top. To measure on your own GPU, run `cargo test --release bench -- --ignored --nocapture --test-threads=1`.

While a game is running, measured over 60 seconds of gameplay in Persona 5 Royal (running at 90 fps) with the effect active and default settings (Quarter, 30 fps):

| nijimi while active | Average | 95th percentile |
|---|---|---|
| CPU | **0.01%** of total CPU | 0.07% |
| GPU (3D engine) | **0.66%** | 0.68% |
| GPU memory | 295 MB | 295 MB |
| Memory | 18 MB | 18 MB |

For scale, the game itself used 10.1% of the GPU over the same period. After Alt-Tabbing out, nijimi's GPU use drops to 0% immediately, and its GPU memory to 0 MB after a minute. GPU cost scales with display resolution and GPU speed, so expect a proportionally higher percentage on slower cards or a lower one at lower resolutions. To measure on your own system, run `scripts/measure-performance.ps1` while playing.

## Getting started

1. Download the latest `nijimi-…-windows-x64.zip` from [Releases](../../releases/latest) (or [build it yourself](#building)) and extract it to a folder you can write to.
2. Run it. The icon appears in the system tray and **Settings** opens.
3. In Settings, choose **Add app…**, and pick your game from recently used apps, running processes, or **Browse…**.
4. Pick an aspect ratio. Changes save automatically.
5. Launch the game. The ambient effect appears in the bars as soon as it's in front.

Closing Settings leaves nijimi running in the tray. Click the tray icon or run nijimi again to reopen Settings, and right-click the tray icon for **Exit**, which also closes Settings.

### Requirements

- Windows 10 version 2004 or later
- A Direct3D 11-capable GPU

Nothing else is needed. `nijimi.exe` is fully self-contained and doesn't need the Visual C++ Redistributable or any other runtime.

## Settings

![nijimi's Program settings: the profile list on the left, sorted alphabetically, and the selected game's aspect ratio and look on the right, with a live preview of the effect](assets/screenshots/program-settings-1.png)

- **Program settings.** Lists your game profiles in alphabetical order. Search by executable name or path, select a profile to change its aspect ratio, give it its own look instead of the global one, or remove it. Supported ratios range from `1:4` to `4:1`; invalid entries are rejected and the previous value is restored.
- **Global settings.** Sets the default aspect ratio for new profiles (initially `21:9`); existing profiles keep their own ratios. The **look** (edge softness, edge blend, blur strength, and blur resolution) set here applies to every profile without its own. **Frame rate** updates the effect 30 (default) or 60 times a second. **Always on** shows the effect whatever app is in front: profiled games still use their own ratio, and everything else uses the default. It's off by default, and while it's on nijimi keeps rendering instead of going idle. **Show in screenshots** lets screenshots and recordings capture the effect; it's off by default, so they normally show plain black bars. **Start with Windows** launches nijimi in the tray when you sign in, without opening Settings. It's off by default and applies to the copy you turn it on from. If you move the executable, turn it off and on again.
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

1. **Capture.** The desktop is captured with DXGI Desktop Duplication. Frames stay in GPU memory and are never copied to the CPU. The latest frame is kept in a texture, and the overlay is only redrawn when the screen or the settings change, so a static screen or a moving mouse costs no GPU work.
2. **Sample.** The content near the edges of the protected rectangle is reflected outward and averaged down to the blur resolution (full, half, quarter, or eighth).
3. **Blur.** A separable Gaussian sized by the blur strength, plus a wider smoothing pass, produces a broad, soft glow, followed by a distance fade away from the content. It's only computed in and around the bars, widened by the blur's reach so the result is identical to blurring the whole screen. At Full resolution the glow still runs at half resolution, where it looks the same at a fraction of the cost; Full sharpens the edge blend. Intermediates are 10-bit and the result half-float, so wide gradients don't band.
4. **Composite.** The full-resolution result is dithered and clipped to the area *outside* the protected rectangle, then presented through a click-through DirectComposition overlay. With edge softness or edge blend on, the overlay also covers the content's outermost few pixels (at most 1% of the screen's shorter side; video and games often have a fringe there), fully opaque at the very edge and fading into the picture. From the start of that band outwards the blur radius grows smoothly from zero to the glow's, read from a mip chain of the prefiltered texture with B-spline filtering, while the band hands over to the glow across its whole width. The glow is as bright as the picture where it meets it and settles to its own level further out, so neither sharpness nor brightness changes at a visible line. The overlay excludes itself from capture by default, and because it never samples anything it draws over, it never feeds back into its own input even when screenshots are allowed to show it.

The protected rectangle is computed with integer geometry, fitted and centred on the primary display for the selected ratio.

### Performance

The project's main goal is minimal impact on the game you're playing.

- **Idle.** When the configured game leaves the foreground, rendering and capture stop immediately and the overlay is hidden. The Direct3D device, swap chain, composition objects, and textures are kept for one minute so a quick return is instant, then all released. Only the hidden overlay window remains.
- **Precompiled shaders.** `build.rs` compiles `src/ambient.hlsl` to bytecode at build time, so the HLSL compiler is never loaded and no shaders are compiled at runtime.
- **Active.** The render loop runs at 30 fps by default (60 if chosen), paced by a high-resolution waitable timer (Windows' standard `SetTimer` can only tick every 15.6 ms, so a 33 ms request actually ran at about 21 fps), entirely on the GPU, and blurs the glow at half resolution or lower. Ticks with no new desktop frame skip drawing. Faster rates aren't offered: presenting the overlay more than about 60 times a second made the graphics driver stall for several milliseconds per frame.
- **Recovery.** If capture access is lost (for example on a UAC prompt or a mode switch), the overlay is cleared and capture is retried with a backoff of up to four seconds. Resources are rebuilt after display changes. If rendering fails, it stops rather than retrying on every window switch, and tries again when settings are saved or the display configuration changes.
- **Process lookup.** If Windows refuses access to a process's path, it's treated as not matching any profile, so the effect stays off.

Gameplay frame-time impact has not yet been formally measured.

### Building

Install the stable Rust toolchain, then:

```powershell
cargo build --release            # optimised build for distribution (full LTO)
cargo run --profile dev-release  # faster-building optimised profile for day-to-day testing
cargo test                       # geometry, settings, activation rules, frame timing, WARP render tests
```

The release profile uses full LTO with a single codegen unit, so the final link step takes a minute or two. The `dev-release` profile uses thin LTO for much faster rebuilds and is intended for testing only. Ship builds from `--release`.

In RustRover, the shared **Run** configuration launches the app with `dev-release`, and **Build** produces the full release executable.

The app icon is embedded as a Windows resource from `assets/tresbient.ico`, and the UI's SVG assets are compiled into the binary. Windows may keep showing a cached icon until Explorer refreshes its icon cache.

### Releasing

The **Build** GitHub Actions workflow tests every push to `main` and every pull request. To publish a release, bump `version` in `Cargo.toml` and push to `main`. If that version has no release yet, the workflow builds it, creates the `v<version>` tag, and publishes a GitHub release with `nijimi-v<version>-windows-x64.zip` (exe, README, and license) and its SHA-256 checksum. Pushes that don't change the version only run the tests.

### Tests

`cargo test` covers aspect-ratio geometry (including portrait and odd-sized displays), settings round trips and activation rules, capture retry behaviour, frame timer accuracy, and offscreen Direct3D WARP rendering of the blur passes, including a check that blurring only around the bars matches blurring the whole screen pixel for pixel.

Two opt-in tests run on your own GPU: `cargo test --release -- --ignored --nocapture --test-threads=1` prints the GPU time per frame across screen sizes, aspect ratios, and blur resolutions, and checks that Full resolution looks the same with its glow at half resolution.

## License

[MIT](LICENSE)
