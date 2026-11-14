//! GPU cost of the ambient pipeline, measured with timestamp queries on this machine's GPU.
//! Run with `cargo test --release bench -- --ignored --nocapture --test-threads=1`.

use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_QUERY, D3D11_QUERY_DATA_TIMESTAMP_DISJOINT, D3D11_QUERY_DESC, D3D11_QUERY_EVENT,
    D3D11_QUERY_TIMESTAMP, D3D11_QUERY_TIMESTAMP_DISJOINT, ID3D11Query,
};

use super::*;
use crate::settings::BlurResolution;

const WARMUP_FRAMES: usize = 60;
const TIMED_FRAMES: usize = 240;

fn query(device: &ID3D11Device, kind: D3D11_QUERY) -> ID3D11Query {
    let mut query = None;
    unsafe {
        device
            .CreateQuery(
                &D3D11_QUERY_DESC {
                    Query: kind,
                    MiscFlags: 0,
                },
                Some(&mut query),
            )
            .unwrap();
    }
    query.unwrap()
}

/// Blocks until the GPU has finished everything submitted so far.
fn finish(context: &ID3D11DeviceContext, event: &ID3D11Query) {
    unsafe {
        context.End(event);
        context.Flush();
        loop {
            let mut done = 0u32;
            let _ = context.GetData(event, Some((&mut done as *mut u32).cast()), 4, 0);
            if done != 0 {
                return;
            }
            std::thread::yield_now();
        }
    }
}

struct Screen {
    width: u32,
    height: u32,
    source: ID3D11ShaderResourceView,
    output: ID3D11RenderTargetView,
}

fn screen(device: &ID3D11Device, width: u32, height: u32) -> Screen {
    // Noise, so texture compression can't make the reads cheaper than a real picture's.
    let mut seed = 0x2545_f491u32;
    let pixels: Vec<u8> = (0..width * height * 4)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    unsafe {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: pixels.as_ptr().cast(),
            SysMemPitch: width * 4,
            SysMemSlicePitch: 0,
        };
        let mut texture = None;
        device
            .CreateTexture2D(&desc, Some(&data), Some(&mut texture))
            .unwrap();
        let mut source = None;
        device
            .CreateShaderResourceView(&texture.unwrap(), None, Some(&mut source))
            .unwrap();
        let mut output = None;
        device
            .CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                    ..desc
                },
                None,
                Some(&mut output),
            )
            .unwrap();
        let mut render_target = None;
        device
            .CreateRenderTargetView(&output.unwrap(), None, Some(&mut render_target))
            .unwrap();
        Screen {
            width,
            height,
            source: source.unwrap(),
            output: render_target.unwrap(),
        }
    }
}

/// Median and 95th percentile GPU milliseconds per frame.
fn gpu_ms(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    shaders: &AmbientShaders,
    screen: &Screen,
    aspect_ratio: AspectRatio,
    effect: EffectSettings,
    overlays: bool,
) -> (f64, f64) {
    let mut scene = SceneGeometry::new(screen.width, screen.height, aspect_ratio, effect).unwrap();
    if overlays {
        scene.constants.overlay_passthrough = 1.0;
    }
    let blur = BlurTargets::new(device, screen.width, screen.height, scene.blur_scale).unwrap();
    let scene_buffer = create_scene_buffer(device).unwrap();
    let draw = || unsafe {
        draw_ambient(AmbientPass {
            context,
            render_target: &screen.output,
            scene_buffer: &scene_buffer,
            scene: &scene,
            blur: &blur,
            source: &screen.source,
            shaders,
            width: screen.width,
            height: screen.height,
        })
        .unwrap();
    };
    let event = query(device, D3D11_QUERY_EVENT);
    for _ in 0..WARMUP_FRAMES {
        draw();
    }
    finish(context, &event);
    let disjoint = query(device, D3D11_QUERY_TIMESTAMP_DISJOINT);
    let stamps: Vec<_> = (0..=TIMED_FRAMES)
        .map(|_| query(device, D3D11_QUERY_TIMESTAMP))
        .collect();
    unsafe {
        context.Begin(&disjoint);
        context.End(&stamps[0]);
        for stamp in &stamps[1..] {
            draw();
            context.End(stamp);
        }
        context.End(&disjoint);
    }
    finish(context, &event);
    let read = |query: &ID3D11Query, data: *mut std::ffi::c_void, size: usize| unsafe {
        context.GetData(query, Some(data), size as u32, 0).unwrap();
    };
    let mut timing = D3D11_QUERY_DATA_TIMESTAMP_DISJOINT::default();
    read(
        &disjoint,
        (&mut timing as *mut D3D11_QUERY_DATA_TIMESTAMP_DISJOINT).cast(),
        size_of::<D3D11_QUERY_DATA_TIMESTAMP_DISJOINT>(),
    );
    assert!(!timing.Disjoint.as_bool(), "the GPU clock changed mid-run");
    let ticks: Vec<u64> = stamps
        .iter()
        .map(|stamp| {
            let mut value = 0u64;
            read(stamp, (&mut value as *mut u64).cast(), 8);
            value
        })
        .collect();
    let mut frames: Vec<f64> = ticks
        .windows(2)
        .map(|pair| (pair[1] - pair[0]) as f64 * 1000.0 / timing.Frequency as f64)
        .collect();
    frames.sort_by(f64::total_cmp);
    (frames[frames.len() / 2], frames[frames.len() * 95 / 100])
}

#[test]
#[ignore]
fn bench_gpu_cost_per_frame() {
    let (device, context) = create_device(None, D3D_DRIVER_TYPE_HARDWARE).unwrap();
    let shaders = AmbientShaders::new(&device).unwrap();
    let looks = [
        ("default", EffectSettings::default(), false),
        (
            "maximum",
            EffectSettings {
                edge_softness: 100,
                edge_blend: 100,
                blur: 100,
                ..EffectSettings::default()
            },
            false,
        ),
        ("overlays", EffectSettings::default(), true),
    ];
    println!("screen     content  look     resolution  median ms  p95 ms");
    for (width, height) in [(5120, 2160), (3840, 2160), (2560, 1440)] {
        let screen = screen(&device, width, height);
        for (ratio_width, ratio_height) in [(16, 9), (4, 3)] {
            let aspect_ratio = AspectRatio::new(ratio_width, ratio_height).unwrap();
            for (name, look, overlays) in looks {
                for resolution in BlurResolution::ALL {
                    let effect = EffectSettings { resolution, ..look };
                    let (median, p95) = gpu_ms(
                        &device,
                        &context,
                        &shaders,
                        &screen,
                        aspect_ratio,
                        effect,
                        overlays,
                    );
                    println!(
                        "{width}x{height}  {ratio_width:>2}:{ratio_height:<2}    {name:<8} {:<10}  {median:>9.3}  {p95:>6.3}",
                        resolution.label()
                    );
                }
            }
        }
    }
}
