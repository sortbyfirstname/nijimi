use std::mem::size_of;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{BOOL, COLORREF, HWND, RECT};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_11_0, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BUFFER_DESC, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_FILTER_MIN_MAG_MIP_LINEAR,
    D3D11_RASTERIZER_DESC, D3D11_SAMPLER_DESC, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA,
    D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_VIEWPORT,
    D3D11CreateDevice, ID3D11Buffer, ID3D11ClassLinkage, ID3D11DepthStencilView, ID3D11Device,
    ID3D11DeviceContext, ID3D11PixelShader, ID3D11RasterizerState, ID3D11RenderTargetView,
    ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_WAIT_TIMEOUT, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    IDXGIAdapter, IDXGIAdapter1, IDXGIDevice, IDXGIFactory2, IDXGIOutput1, IDXGIOutputDuplication,
    IDXGISwapChain1,
};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, LWA_ALPHA, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowPos, ShowWindow,
    WDA_EXCLUDEFROMCAPTURE, WINDOW_DISPLAY_AFFINITY, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
};
use windows::core::{Error, Interface, Result, w};

use crate::geometry::{AspectRatio, ScreenRect, ambient_regions, content_bounds, fit_content};

const WINDOW_CLASS: windows::core::PCWSTR = w!("NijimiOverlayWindow");
const BLUR_SCALE: u32 = 4;
const CAPTURE_RETRY_INITIAL: Duration = Duration::from_millis(250);
const CAPTURE_RETRY_MAX: Duration = Duration::from_secs(4);
const INITIAL_FRAME_TIMEOUT_MS: u32 = 100;
static OVERLAY_CLASS_REGISTERED: OnceLock<()> = OnceLock::new();

/// The overlay window plus, only while a configured app is active, the GPU resources that draw
/// into it. Keeping the GPU side optional means an idle tray app holds no D3D device, swap chain,
/// or full-resolution textures.
pub struct Renderer {
    hwnd: HWND,
    width: u32,
    height: u32,
    enabled: bool,
    aspect_ratio: AspectRatio,
    scene: SceneGeometry,
    gpu: Option<Gpu>,
    _window: OverlayWindow,
}

struct Gpu {
    width: u32,
    height: u32,
    capture_retry: CaptureRetry,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    duplication: Option<IDXGIOutputDuplication>,
    swap_chain: IDXGISwapChain1,
    _target: IDCompositionTarget,
    _composition: IDCompositionDevice,
    render_target: ID3D11RenderTargetView,
    scene_buffer: ID3D11Buffer,
    blur_render_target: ID3D11RenderTargetView,
    blur_view: ID3D11ShaderResourceView,
    prefilter_render_target: ID3D11RenderTargetView,
    prefilter_view: ID3D11ShaderResourceView,
    horizontal_render_target: ID3D11RenderTargetView,
    horizontal_view: ID3D11ShaderResourceView,
    wide_horizontal_render_target: ID3D11RenderTargetView,
    wide_horizontal_view: ID3D11ShaderResourceView,
    wide_blur_render_target: ID3D11RenderTargetView,
    wide_blur_view: ID3D11ShaderResourceView,
    blur_width: u32,
    blur_height: u32,
    captured_texture: ID3D11Texture2D,
    captured_view: ID3D11ShaderResourceView,
    has_captured_frame: bool,
    vertex_shader: ID3D11VertexShader,
    prepare_pixel_shader: ID3D11PixelShader,
    horizontal_pixel_shader: ID3D11PixelShader,
    vertical_pixel_shader: ID3D11PixelShader,
    wide_horizontal_pixel_shader: ID3D11PixelShader,
    wide_vertical_pixel_shader: ID3D11PixelShader,
    blur_pixel_shader: ID3D11PixelShader,
    composite_pixel_shader: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    rasterizer: ID3D11RasterizerState,
}

struct OverlayWindow(HWND);

#[repr(C)]
#[derive(Clone, Copy)]
struct SceneConstants {
    content_rect: [f32; 4],
}

impl Default for SceneConstants {
    fn default() -> Self {
        Self {
            content_rect: [0.0, 0.0, 1.0, 1.0],
        }
    }
}

struct SceneGeometry {
    rect: ScreenRect,
    constants: SceneConstants,
}
impl SceneGeometry {
    fn new(width: u32, height: u32, aspect_ratio: AspectRatio) -> Option<Self> {
        let rect = fit_content(width, height, aspect_ratio)?;
        let bounds = content_bounds(rect, width, height)?;
        Some(Self {
            rect,
            constants: SceneConstants {
                content_rect: [bounds.left, bounds.top, bounds.right, bounds.bottom],
            },
        })
    }
}

struct CaptureRetry {
    retry_at: Option<Instant>,
    delay: Duration,
}

impl Default for CaptureRetry {
    fn default() -> Self {
        Self {
            retry_at: None,
            delay: CAPTURE_RETRY_INITIAL,
        }
    }
}

impl CaptureRetry {
    fn is_ready(&self) -> bool {
        self.retry_at
            .is_none_or(|retry_at| Instant::now() >= retry_at)
    }

    fn schedule(&mut self) {
        self.retry_at = Some(Instant::now() + self.delay);
        self.delay = (self.delay * 2).min(CAPTURE_RETRY_MAX);
    }

    fn reset(&mut self) {
        self.retry_at = None;
        self.delay = CAPTURE_RETRY_INITIAL;
    }
}

fn capture_timeout_ms(has_captured_frame: bool) -> u32 {
    if has_captured_frame {
        0
    } else {
        INITIAL_FRAME_TIMEOUT_MS
    }
}

unsafe fn create_blur_target(
    device: &ID3D11Device,
    desc: &D3D11_TEXTURE2D_DESC,
) -> Result<(ID3D11RenderTargetView, ID3D11ShaderResourceView)> {
    unsafe {
        let mut texture = None;
        device.CreateTexture2D(desc, None, Some(&mut texture))?;
        let texture: ID3D11Texture2D = texture.ok_or_else(Error::from_win32)?;
        let mut render_target = None;
        device.CreateRenderTargetView(&texture, None, Some(&mut render_target))?;
        let render_target = render_target.ok_or_else(Error::from_win32)?;
        let mut view = None;
        device.CreateShaderResourceView(&texture, None, Some(&mut view))?;
        let view = view.ok_or_else(Error::from_win32)?;
        Ok((render_target, view))
    }
}

struct AmbientPass<'a> {
    context: &'a ID3D11DeviceContext,
    render_target: &'a ID3D11RenderTargetView,
    scene_buffer: &'a ID3D11Buffer,
    scene: &'a SceneGeometry,
    blur_render_target: &'a ID3D11RenderTargetView,
    blur_view: &'a ID3D11ShaderResourceView,
    prefilter_render_target: &'a ID3D11RenderTargetView,
    prefilter_view: &'a ID3D11ShaderResourceView,
    horizontal_render_target: &'a ID3D11RenderTargetView,
    horizontal_view: &'a ID3D11ShaderResourceView,
    wide_horizontal_render_target: &'a ID3D11RenderTargetView,
    wide_horizontal_view: &'a ID3D11ShaderResourceView,
    wide_blur_render_target: &'a ID3D11RenderTargetView,
    wide_blur_view: &'a ID3D11ShaderResourceView,
    source: &'a ID3D11ShaderResourceView,
    vertex_shader: &'a ID3D11VertexShader,
    prepare_pixel_shader: &'a ID3D11PixelShader,
    horizontal_pixel_shader: &'a ID3D11PixelShader,
    vertical_pixel_shader: &'a ID3D11PixelShader,
    wide_horizontal_pixel_shader: &'a ID3D11PixelShader,
    wide_vertical_pixel_shader: &'a ID3D11PixelShader,
    blur_pixel_shader: &'a ID3D11PixelShader,
    composite_pixel_shader: &'a ID3D11PixelShader,
    sampler: &'a ID3D11SamplerState,
    rasterizer: &'a ID3D11RasterizerState,
    width: u32,
    height: u32,
    blur_width: u32,
    blur_height: u32,
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0);
        }
    }
}

impl Renderer {
    pub fn new() -> Result<Self> {
        Self::with_aspect_ratio(AspectRatio::new(16, 9).unwrap())
    }

    fn with_aspect_ratio(aspect_ratio: AspectRatio) -> Result<Self> {
        unsafe {
            let width = windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                windows::Win32::UI::WindowsAndMessaging::SM_CXSCREEN,
            ) as u32;
            let height = windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                windows::Win32::UI::WindowsAndMessaging::SM_CYSCREEN,
            ) as u32;
            if width == 0 || height == 0 {
                return Err(Error::from_win32());
            }
            let scene =
                SceneGeometry::new(width, height, aspect_ratio).ok_or_else(Error::from_win32)?;

            let instance = windows::Win32::Foundation::HINSTANCE(GetModuleHandleW(None)?.0);
            let class = WNDCLASSW {
                lpfnWndProc: Some(overlay_window_proc),
                hInstance: instance,
                lpszClassName: WINDOW_CLASS,
                ..Default::default()
            };
            if OVERLAY_CLASS_REGISTERED.get().is_none() {
                if RegisterClassW(&class) == 0 {
                    return Err(Error::from_win32());
                }
                let _ = OVERLAY_CLASS_REGISTERED.set(());
            }
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(
                    windows::Win32::UI::WindowsAndMessaging::WS_EX_TOPMOST.0
                        | windows::Win32::UI::WindowsAndMessaging::WS_EX_TRANSPARENT.0
                        | windows::Win32::UI::WindowsAndMessaging::WS_EX_NOACTIVATE.0
                        | windows::Win32::UI::WindowsAndMessaging::WS_EX_TOOLWINDOW.0
                        | windows::Win32::UI::WindowsAndMessaging::WS_EX_LAYERED.0,
                ),
                WINDOW_CLASS,
                w!("nijimi ambient overlay"),
                WINDOW_STYLE(windows::Win32::UI::WindowsAndMessaging::WS_POPUP.0),
                0,
                0,
                width as i32,
                height as i32,
                HWND::default(),
                windows::Win32::UI::WindowsAndMessaging::HMENU::default(),
                instance,
                None,
            )?;
            let window = OverlayWindow(hwnd);
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
            SetWindowDisplayAffinity(hwnd, WINDOW_DISPLAY_AFFINITY(WDA_EXCLUDEFROMCAPTURE.0))?;
            SetWindowPos(
                hwnd,
                windows::Win32::UI::WindowsAndMessaging::HWND_TOPMOST,
                0,
                0,
                width as i32,
                height as i32,
                SWP_NOACTIVATE,
            )?;

            Ok(Self {
                hwnd,
                width,
                height,
                enabled: false,
                aspect_ratio,
                scene,
                gpu: None,
                _window: window,
            })
        }
    }

    pub fn set_aspect_ratio(&mut self, aspect_ratio: AspectRatio) -> Result<()> {
        self.scene = SceneGeometry::new(self.width, self.height, aspect_ratio)
            .ok_or_else(Error::from_win32)?;
        self.aspect_ratio = aspect_ratio;
        Ok(())
    }

    pub fn set_enabled(&mut self, enabled: bool) -> Result<()> {
        if enabled {
            if self.gpu.is_none() {
                self.gpu = Some(Gpu::new(self.hwnd, self.width, self.height)?);
            }
            let gpu = self.gpu.as_mut().unwrap();
            if gpu.duplication.is_none() {
                gpu.duplication = Some(unsafe { gpu.output.DuplicateOutput(&gpu.device)? });
            }
            self.enabled = true;
            let _ = unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
        } else {
            self.enabled = false;
            // Hide first so the last frame is never visible, then release every GPU resource.
            // The next activation builds a fresh swap chain that starts cleared.
            let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            self.gpu = None;
        }
        Ok(())
    }

    /// Hides the overlay and stops capturing, but keeps the GPU resources so a quick return to
    /// the game (e.g. Alt-Tab and back) skips the ~75 ms driver device creation. Call
    /// [`Self::release_gpu`] once the pause has lasted long enough to be worth freeing them.
    pub fn pause(&mut self) -> Result<()> {
        self.enabled = false;
        let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        gpu.duplication = None;
        gpu.has_captured_frame = false;
        // Clear so the previous game's frame never flashes when the overlay is shown again.
        if let Err(error) = gpu.clear_overlay() {
            self.gpu = None;
            return Err(error);
        }
        Ok(())
    }

    /// Frees all GPU resources if the overlay is not currently active.
    pub fn release_gpu(&mut self) {
        if !self.enabled {
            self.gpu = None;
        }
    }

    pub fn refresh(&mut self, enabled: bool) -> Result<()> {
        let _ = self.set_enabled(false);
        let mut replacement = Self::with_aspect_ratio(self.aspect_ratio)?;
        replacement.set_enabled(enabled)?;
        *self = replacement;
        Ok(())
    }

    pub fn render(&mut self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let width = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                windows::Win32::UI::WindowsAndMessaging::SM_CXSCREEN,
            ) as u32
        };
        let height = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                windows::Win32::UI::WindowsAndMessaging::SM_CYSCREEN,
            ) as u32
        };
        if width != self.width || height != self.height {
            return self.refresh(true);
        }
        match self.gpu.as_mut() {
            Some(gpu) => gpu.render(&self.scene),
            None => Ok(()),
        }
    }
}

impl Gpu {
    fn new(hwnd: HWND, width: u32, height: u32) -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
            let (adapter, output) = find_primary_output(&factory)?;
            let d3d_adapter: IDXGIAdapter = adapter.cast()?;
            let mut device = None;
            let mut context = None;
            let mut feature_level = D3D_FEATURE_LEVEL_11_0;
            D3D11CreateDevice(
                &d3d_adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[
                    D3D_FEATURE_LEVEL_11_0,
                    D3D_FEATURE_LEVEL_10_1,
                    D3D_FEATURE_LEVEL_10_0,
                ]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut feature_level),
                Some(&mut context),
            )?;
            let device = device.ok_or_else(Error::from_win32)?;
            let context = context.ok_or_else(Error::from_win32)?;

            let captured_desc = D3D11_TEXTURE2D_DESC {
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
            let mut captured_texture = None;
            device.CreateTexture2D(&captured_desc, None, Some(&mut captured_texture))?;
            let captured_texture: ID3D11Texture2D =
                captured_texture.ok_or_else(Error::from_win32)?;
            let mut captured_view = None;
            device.CreateShaderResourceView(&captured_texture, None, Some(&mut captured_view))?;
            let captured_view = captured_view.ok_or_else(Error::from_win32)?;

            let dxgi_device: IDXGIDevice = device.cast()?;
            let swap_desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: BOOL(0),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                Flags: 0,
            };
            let swap_chain = factory.CreateSwapChainForComposition(&device, &swap_desc, None)?;
            let composition: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;
            let target = composition.CreateTargetForHwnd(hwnd, BOOL(1))?;
            let visual = composition.CreateVisual()?;
            visual.SetContent(&swap_chain)?;
            target.SetRoot(&visual)?;
            composition.Commit()?;

            let back_buffer: ID3D11Texture2D = swap_chain.GetBuffer(0)?;
            let mut render_target = None;
            device.CreateRenderTargetView(&back_buffer, None, Some(&mut render_target))?;
            let render_target = render_target.ok_or_else(Error::from_win32)?;
            let initial_scene = SceneConstants::default();
            let scene_data = D3D11_SUBRESOURCE_DATA {
                pSysMem: (&initial_scene as *const SceneConstants).cast(),
                SysMemPitch: 0,
                SysMemSlicePitch: 0,
            };
            let scene_desc = D3D11_BUFFER_DESC {
                ByteWidth: size_of::<SceneConstants>() as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
                StructureByteStride: 0,
            };
            let mut scene_buffer = None;
            device.CreateBuffer(&scene_desc, Some(&scene_data), Some(&mut scene_buffer))?;
            let scene_buffer = scene_buffer.ok_or_else(Error::from_win32)?;
            let blur_width = width.div_ceil(BLUR_SCALE);
            let blur_height = height.div_ceil(BLUR_SCALE);
            let blur_desc = D3D11_TEXTURE2D_DESC {
                Width: blur_width,
                Height: blur_height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32 | D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let (blur_render_target, blur_view) = create_blur_target(&device, &blur_desc)?;
            let (prefilter_render_target, prefilter_view) =
                create_blur_target(&device, &blur_desc)?;
            let (horizontal_render_target, horizontal_view) =
                create_blur_target(&device, &blur_desc)?;
            let (wide_horizontal_render_target, wide_horizontal_view) =
                create_blur_target(&device, &blur_desc)?;
            let (wide_blur_render_target, wide_blur_view) =
                create_blur_target(&device, &blur_desc)?;
            context.ClearRenderTargetView(&render_target, &[0.0, 0.0, 0.0, 0.0]);
            swap_chain.Present(0, DXGI_PRESENT(0)).ok()?;
            let vertex_shader = create_vertex_shader(&device)?;
            let prepare_pixel_shader = create_pixel_shader(&device, PS_PREPARE)?;
            let horizontal_pixel_shader = create_pixel_shader(&device, PS_BLUR_HORIZONTAL)?;
            let vertical_pixel_shader = create_pixel_shader(&device, PS_BLUR_VERTICAL)?;
            let wide_horizontal_pixel_shader =
                create_pixel_shader(&device, PS_BLUR_WIDE_HORIZONTAL)?;
            let wide_vertical_pixel_shader = create_pixel_shader(&device, PS_BLUR_WIDE_VERTICAL)?;
            let blur_pixel_shader = create_pixel_shader(&device, PS_BLUR)?;
            let composite_pixel_shader = create_pixel_shader(&device, PS_COMPOSITE)?;
            let sampler_desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MipLODBias: 0.0,
                MaxAnisotropy: 1,
                ComparisonFunc: windows::Win32::Graphics::Direct3D11::D3D11_COMPARISON_NEVER,
                BorderColor: [0.0; 4],
                MinLOD: 0.0,
                MaxLOD: f32::MAX,
            };
            let mut sampler = None;
            device.CreateSamplerState(&sampler_desc, Some(&mut sampler))?;
            let sampler = sampler.ok_or_else(Error::from_win32)?;
            let rasterizer_desc = D3D11_RASTERIZER_DESC {
                FillMode: windows::Win32::Graphics::Direct3D11::D3D11_FILL_SOLID,
                CullMode: windows::Win32::Graphics::Direct3D11::D3D11_CULL_NONE,
                FrontCounterClockwise: BOOL(0),
                DepthBias: 0,
                DepthBiasClamp: 0.0,
                SlopeScaledDepthBias: 0.0,
                DepthClipEnable: BOOL(1),
                ScissorEnable: BOOL(1),
                MultisampleEnable: BOOL(0),
                AntialiasedLineEnable: BOOL(0),
            };
            let mut rasterizer = None;
            device.CreateRasterizerState(&rasterizer_desc, Some(&mut rasterizer))?;
            let rasterizer = rasterizer.ok_or_else(Error::from_win32)?;

            Ok(Self {
                width,
                height,
                device,
                context,
                output,
                duplication: None,
                capture_retry: CaptureRetry::default(),
                swap_chain,
                _target: target,
                _composition: composition,
                render_target,
                scene_buffer,
                blur_render_target,
                blur_view,
                prefilter_render_target,
                prefilter_view,
                horizontal_render_target,
                horizontal_view,
                wide_horizontal_render_target,
                wide_horizontal_view,
                wide_blur_render_target,
                wide_blur_view,
                blur_width,
                blur_height,
                captured_texture,
                captured_view,
                has_captured_frame: false,
                vertex_shader,
                prepare_pixel_shader,
                horizontal_pixel_shader,
                vertical_pixel_shader,
                wide_horizontal_pixel_shader,
                wide_vertical_pixel_shader,
                blur_pixel_shader,
                composite_pixel_shader,
                sampler,
                rasterizer,
            })
        }
    }

    fn render(&mut self, scene: &SceneGeometry) -> Result<()> {
        if self.duplication.is_none() {
            if !self.capture_retry.is_ready() {
                return Ok(());
            }
            match unsafe { self.output.DuplicateOutput(&self.device) } {
                Ok(duplication) => {
                    self.duplication = Some(duplication);
                    self.capture_retry.reset();
                }
                Err(_) => {
                    self.capture_retry.schedule();
                    return Ok(());
                }
            }
        }
        let duplication = self.duplication.as_ref().unwrap();
        let mut frame_info = windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        if let Err(error) = unsafe {
            duplication.AcquireNextFrame(
                capture_timeout_ms(self.has_captured_frame),
                &mut frame_info,
                &mut resource,
            )
        } {
            if error.code() == DXGI_ERROR_WAIT_TIMEOUT {
                return if self.has_captured_frame {
                    unsafe { self.draw(scene, &self.captured_view) }
                } else {
                    Ok(())
                };
            }
            self.duplication = None;
            self.has_captured_frame = false;
            self.capture_retry.schedule();
            return self.clear_overlay();
        }

        let frame_result: Result<()> = unsafe {
            let texture: ID3D11Texture2D = resource.ok_or_else(Error::from_win32)?.cast()?;
            self.context.CopyResource(&self.captured_texture, &texture);
            self.has_captured_frame = true;
            Ok(())
        };
        let release_result = unsafe { duplication.ReleaseFrame() };
        frame_result?;
        release_result?;
        unsafe { self.draw(scene, &self.captured_view) }
    }

    unsafe fn draw(&self, scene: &SceneGeometry, source: &ID3D11ShaderResourceView) -> Result<()> {
        unsafe {
            draw_ambient(AmbientPass {
                context: &self.context,
                render_target: &self.render_target,
                scene_buffer: &self.scene_buffer,
                scene,
                blur_render_target: &self.blur_render_target,
                blur_view: &self.blur_view,
                prefilter_render_target: &self.prefilter_render_target,
                prefilter_view: &self.prefilter_view,
                horizontal_render_target: &self.horizontal_render_target,
                horizontal_view: &self.horizontal_view,
                wide_horizontal_render_target: &self.wide_horizontal_render_target,
                wide_horizontal_view: &self.wide_horizontal_view,
                wide_blur_render_target: &self.wide_blur_render_target,
                wide_blur_view: &self.wide_blur_view,
                source,
                vertex_shader: &self.vertex_shader,
                prepare_pixel_shader: &self.prepare_pixel_shader,
                horizontal_pixel_shader: &self.horizontal_pixel_shader,
                vertical_pixel_shader: &self.vertical_pixel_shader,
                wide_horizontal_pixel_shader: &self.wide_horizontal_pixel_shader,
                wide_vertical_pixel_shader: &self.wide_vertical_pixel_shader,
                blur_pixel_shader: &self.blur_pixel_shader,
                composite_pixel_shader: &self.composite_pixel_shader,
                sampler: &self.sampler,
                rasterizer: &self.rasterizer,
                width: self.width,
                height: self.height,
                blur_width: self.blur_width,
                blur_height: self.blur_height,
            })?;
            self.swap_chain.Present(0, DXGI_PRESENT(0)).ok()?;
        }
        Ok(())
    }

    fn clear_overlay(&self) -> Result<()> {
        unsafe {
            self.context
                .ClearRenderTargetView(&self.render_target, &[0.0, 0.0, 0.0, 0.0]);
            self.swap_chain.Present(0, DXGI_PRESENT(0)).ok()?;
        }
        Ok(())
    }
}

unsafe fn draw_ambient(pass: AmbientPass<'_>) -> Result<()> {
    let AmbientPass {
        context,
        render_target,
        scene_buffer,
        scene,
        blur_render_target,
        blur_view,
        prefilter_render_target,
        prefilter_view,
        horizontal_render_target,
        horizontal_view,
        wide_horizontal_render_target,
        wide_horizontal_view,
        wide_blur_render_target,
        wide_blur_view,
        source,
        vertex_shader,
        prepare_pixel_shader,
        horizontal_pixel_shader,
        vertical_pixel_shader,
        wide_horizontal_pixel_shader,
        wide_vertical_pixel_shader,
        blur_pixel_shader,
        composite_pixel_shader,
        sampler,
        rasterizer,
        width,
        height,
        blur_width,
        blur_height,
    } = pass;
    unsafe {
        context.UpdateSubresource(
            scene_buffer,
            0,
            None,
            &scene.constants as *const SceneConstants as *const std::ffi::c_void,
            0,
            0,
        );
        let regions = ambient_regions(scene.rect, width, height);
        let screen_scissors = regions.map(|region| RECT {
            left: region.left,
            top: region.top,
            right: region.right,
            bottom: region.bottom,
        });
        let blur_screen = [RECT {
            left: 0,
            top: 0,
            right: blur_width as i32,
            bottom: blur_height as i32,
        }];
        let pipeline = FullscreenPipeline {
            context,
            vertex_shader,
            scene_buffer,
            sampler,
            rasterizer,
        };
        context.ClearRenderTargetView(prefilter_render_target, &[0.0, 0.0, 0.0, 0.0]);
        draw_fullscreen_pass(
            &pipeline,
            FullscreenPass {
                render_target: prefilter_render_target,
                width: blur_width,
                height: blur_height,
                pixel_shader: prepare_pixel_shader,
                sources: [Some(source.clone()), None, None, None],
                scissors: &blur_screen,
            },
        );
        draw_fullscreen_pass(
            &pipeline,
            FullscreenPass {
                render_target: horizontal_render_target,
                width: blur_width,
                height: blur_height,
                pixel_shader: horizontal_pixel_shader,
                sources: [None, Some(prefilter_view.clone()), None, None],
                scissors: &blur_screen,
            },
        );
        draw_fullscreen_pass(
            &pipeline,
            FullscreenPass {
                render_target: blur_render_target,
                width: blur_width,
                height: blur_height,
                pixel_shader: vertical_pixel_shader,
                sources: [None, Some(horizontal_view.clone()), None, None],
                scissors: &blur_screen,
            },
        );
        draw_fullscreen_pass(
            &pipeline,
            FullscreenPass {
                render_target: wide_horizontal_render_target,
                width: blur_width,
                height: blur_height,
                pixel_shader: wide_horizontal_pixel_shader,
                sources: [None, Some(blur_view.clone()), None, None],
                scissors: &blur_screen,
            },
        );
        draw_fullscreen_pass(
            &pipeline,
            FullscreenPass {
                render_target: wide_blur_render_target,
                width: blur_width,
                height: blur_height,
                pixel_shader: wide_vertical_pixel_shader,
                sources: [None, Some(wide_horizontal_view.clone()), None, None],
                scissors: &blur_screen,
            },
        );

        context.ClearRenderTargetView(prefilter_render_target, &[0.0, 0.0, 0.0, 0.0]);
        draw_fullscreen_pass(
            &pipeline,
            FullscreenPass {
                render_target: prefilter_render_target,
                width: blur_width,
                height: blur_height,
                pixel_shader: blur_pixel_shader,
                sources: [
                    Some(source.clone()),
                    Some(wide_blur_view.clone()),
                    Some(blur_view.clone()),
                    None,
                ],
                scissors: &blur_screen,
            },
        );

        context.ClearRenderTargetView(render_target, &[0.0, 0.0, 0.0, 0.0]);
        draw_fullscreen_pass(
            &pipeline,
            FullscreenPass {
                render_target,
                width,
                height,
                pixel_shader: composite_pixel_shader,
                sources: [
                    Some(source.clone()),
                    Some(prefilter_view.clone()),
                    None,
                    None,
                ],
                scissors: &screen_scissors,
            },
        );
    }
    Ok(())
}

struct FullscreenPipeline<'a> {
    context: &'a ID3D11DeviceContext,
    vertex_shader: &'a ID3D11VertexShader,
    scene_buffer: &'a ID3D11Buffer,
    sampler: &'a ID3D11SamplerState,
    rasterizer: &'a ID3D11RasterizerState,
}

struct FullscreenPass<'a> {
    render_target: &'a ID3D11RenderTargetView,
    width: u32,
    height: u32,
    pixel_shader: &'a ID3D11PixelShader,
    sources: [Option<ID3D11ShaderResourceView>; 4],
    scissors: &'a [RECT],
}

unsafe fn draw_fullscreen_pass(pipeline: &FullscreenPipeline<'_>, pass: FullscreenPass<'_>) {
    unsafe {
        pipeline.context.OMSetRenderTargets(
            Some(&[Some(pass.render_target.clone())]),
            None::<&ID3D11DepthStencilView>,
        );
        pipeline.context.RSSetViewports(Some(&[D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: pass.width as f32,
            Height: pass.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        }]));
        pipeline.context.RSSetState(pipeline.rasterizer);
        pipeline
            .context
            .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        pipeline.context.VSSetShader(pipeline.vertex_shader, None);
        pipeline.context.PSSetShader(pass.pixel_shader, None);
        pipeline
            .context
            .PSSetShaderResources(0, Some(&pass.sources));
        pipeline
            .context
            .PSSetSamplers(0, Some(&[Some(pipeline.sampler.clone())]));
        pipeline
            .context
            .PSSetConstantBuffers(0, Some(&[Some(pipeline.scene_buffer.clone())]));
        for scissor in pass.scissors {
            if scissor.left < scissor.right && scissor.top < scissor.bottom {
                pipeline
                    .context
                    .RSSetScissorRects(Some(std::slice::from_ref(scissor)));
                pipeline.context.Draw(3, 0);
            }
        }
        pipeline
            .context
            .PSSetShaderResources(0, Some(&[None, None, None, None]));
    }
}

unsafe extern "system" fn overlay_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn find_primary_output(factory: &IDXGIFactory2) -> Result<(IDXGIAdapter1, IDXGIOutput1)> {
    unsafe {
        let primary = MonitorFromPoint(
            windows::Win32::Foundation::POINT { x: 0, y: 0 },
            MONITOR_DEFAULTTOPRIMARY,
        );
        for adapter_index in 0..16 {
            let Ok(adapter) = factory.EnumAdapters1(adapter_index) else {
                break;
            };
            for output_index in 0..16 {
                let Ok(output) = adapter.EnumOutputs(output_index) else {
                    break;
                };
                let desc = output.GetDesc()?;
                if desc.Monitor == primary {
                    return Ok((adapter, output.cast()?));
                }
            }
        }
    }
    Err(Error::from_win32())
}

/// Shader bytecode compiled from `ambient.hlsl` by `build.rs`.
macro_rules! shader_bytecode {
    ($entry:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/", $entry, ".cso"))
    };
}

const VS_MAIN: &[u8] = shader_bytecode!("VSMain");
const PS_PREPARE: &[u8] = shader_bytecode!("PSPrepare");
const PS_BLUR_HORIZONTAL: &[u8] = shader_bytecode!("PSBlurHorizontal");
const PS_BLUR_VERTICAL: &[u8] = shader_bytecode!("PSBlurVertical");
const PS_BLUR_WIDE_HORIZONTAL: &[u8] = shader_bytecode!("PSBlurWideHorizontal");
const PS_BLUR_WIDE_VERTICAL: &[u8] = shader_bytecode!("PSBlurWideVertical");
const PS_BLUR: &[u8] = shader_bytecode!("PSBlur");
const PS_COMPOSITE: &[u8] = shader_bytecode!("PSComposite");

unsafe fn create_vertex_shader(device: &ID3D11Device) -> Result<ID3D11VertexShader> {
    let mut shader = None;
    unsafe {
        device.CreateVertexShader(VS_MAIN, None::<&ID3D11ClassLinkage>, Some(&mut shader))?;
    }
    shader.ok_or_else(Error::from_win32)
}

unsafe fn create_pixel_shader(device: &ID3D11Device, bytecode: &[u8]) -> Result<ID3D11PixelShader> {
    let mut shader = None;
    unsafe {
        device.CreatePixelShader(bytecode, None::<&ID3D11ClassLinkage>, Some(&mut shader))?;
    }
    shader.ok_or_else(Error::from_win32)
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::*;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ,
        D3D11_MAP_READ, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
        D3D11_USAGE_STAGING, D3D11CreateDevice,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

    const WIDTH: u32 = 40;
    const HEIGHT: u32 = 24;

    #[derive(Clone, Copy, Default)]
    struct TestPattern {
        frame: Option<([u8; 4], u32)>,
        striped_content: bool,
        red_box: Option<ScreenRect>,
        spatial_gradient: bool,
    }

    fn test_scene(rect: ScreenRect) -> SceneGeometry {
        let content = content_bounds(rect, WIDTH, HEIGHT).unwrap();
        SceneGeometry {
            rect,
            constants: SceneConstants {
                content_rect: [content.left, content.top, content.right, content.bottom],
            },
        }
    }

    fn render_scene(
        content: ScreenRect,
        bar_color: [u8; 4],
        content_color: [u8; 4],
        bright_object: bool,
    ) -> Result<Vec<[u8; 4]>> {
        render_scene_with_frame(
            content,
            bar_color,
            content_color,
            bright_object,
            TestPattern::default(),
        )
    }

    fn render_scene_with_frame(
        content: ScreenRect,
        bar_color: [u8; 4],
        content_color: [u8; 4],
        bright_object: bool,
        pattern: TestPattern,
    ) -> Result<Vec<[u8; 4]>> {
        unsafe {
            let mut device = None;
            let mut context = None;
            let mut feature_level = D3D_FEATURE_LEVEL_11_0;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[
                    D3D_FEATURE_LEVEL_11_0,
                    D3D_FEATURE_LEVEL_10_1,
                    D3D_FEATURE_LEVEL_10_0,
                ]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut feature_level),
                Some(&mut context),
            )?;
            let device = device.ok_or_else(Error::from_win32)?;
            let context = context.ok_or_else(Error::from_win32)?;

            let mut pixels = Vec::with_capacity((WIDTH * HEIGHT) as usize);
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    let in_content = (x as i32) >= content.left
                        && (x as i32) < content.right
                        && (y as i32) >= content.top
                        && (y as i32) < content.bottom;
                    let pixel = if bright_object && !in_content && x == 2 && y == HEIGHT / 2 {
                        [0, 255, 0, 255]
                    } else if in_content {
                        if pattern.spatial_gradient {
                            let horizontal = (x as i32 - content.left) as f32
                                / (content.right - content.left - 1).max(1) as f32;
                            let vertical = (content.bottom - 1 - y as i32) as f32
                                / (content.bottom - content.top - 1).max(1) as f32;
                            [
                                (20.0 + horizontal * 180.0) as u8,
                                (20.0 + vertical * 180.0) as u8,
                                40,
                                255,
                            ]
                        } else if let Some(red_box) = pattern.red_box
                            && (x as i32) >= red_box.left
                            && (x as i32) < red_box.right
                            && (y as i32) >= red_box.top
                            && (y as i32) < red_box.bottom
                        {
                            [240, 20, 15, 255]
                        } else if let Some((frame_color, frame_width)) = pattern.frame {
                            let edge_distance = (x as i32 - content.left)
                                .min(content.right - 1 - x as i32)
                                .min(y as i32 - content.top)
                                .min(content.bottom - 1 - y as i32);
                            if edge_distance < frame_width as i32 {
                                frame_color
                            } else if pattern.striped_content
                                && ((x as i32 - content.left) / 4) % 2 == 1
                            {
                                [230, 45, 20, 255]
                            } else {
                                content_color
                            }
                        } else if pattern.striped_content
                            && ((x as i32 - content.left) / 4) % 2 == 1
                        {
                            [230, 45, 20, 255]
                        } else {
                            content_color
                        }
                    } else {
                        bar_color
                    };
                    pixels.push(pixel);
                }
            }
            let texture_desc = D3D11_TEXTURE2D_DESC {
                Width: WIDTH,
                Height: HEIGHT,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let initial_data = D3D11_SUBRESOURCE_DATA {
                pSysMem: pixels.as_ptr().cast(),
                SysMemPitch: WIDTH * size_of::<[u8; 4]>() as u32,
                SysMemSlicePitch: 0,
            };
            let mut source_texture = None;
            device.CreateTexture2D(
                &texture_desc,
                Some(&initial_data),
                Some(&mut source_texture),
            )?;
            let source_texture: ID3D11Texture2D = source_texture.ok_or_else(Error::from_win32)?;
            let mut source_view = None;
            device.CreateShaderResourceView(&source_texture, None, Some(&mut source_view))?;
            let source_view = source_view.ok_or_else(Error::from_win32)?;

            let output_desc = D3D11_TEXTURE2D_DESC {
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                ..texture_desc
            };
            let mut output_texture = None;
            device.CreateTexture2D(&output_desc, None, Some(&mut output_texture))?;
            let output_texture: ID3D11Texture2D = output_texture.ok_or_else(Error::from_win32)?;
            let mut render_target = None;
            device.CreateRenderTargetView(&output_texture, None, Some(&mut render_target))?;
            let render_target = render_target.ok_or_else(Error::from_win32)?;

            let blur_width = WIDTH.div_ceil(BLUR_SCALE);
            let blur_height = HEIGHT.div_ceil(BLUR_SCALE);
            let blur_desc = D3D11_TEXTURE2D_DESC {
                Width: blur_width,
                Height: blur_height,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32 | D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..texture_desc
            };
            let (blur_render_target, blur_view) = create_blur_target(&device, &blur_desc)?;
            let (prefilter_render_target, prefilter_view) =
                create_blur_target(&device, &blur_desc)?;
            let (horizontal_render_target, horizontal_view) =
                create_blur_target(&device, &blur_desc)?;
            let (wide_horizontal_render_target, wide_horizontal_view) =
                create_blur_target(&device, &blur_desc)?;
            let (wide_blur_render_target, wide_blur_view) =
                create_blur_target(&device, &blur_desc)?;
            let scene = test_scene(content);
            let scene_data = D3D11_SUBRESOURCE_DATA {
                pSysMem: (&scene.constants as *const SceneConstants).cast(),
                SysMemPitch: 0,
                SysMemSlicePitch: 0,
            };
            let scene_desc = D3D11_BUFFER_DESC {
                ByteWidth: size_of::<SceneConstants>() as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
                StructureByteStride: 0,
            };
            let mut scene_buffer = None;
            device.CreateBuffer(&scene_desc, Some(&scene_data), Some(&mut scene_buffer))?;
            let scene_buffer = scene_buffer.ok_or_else(Error::from_win32)?;

            let sampler_desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MipLODBias: 0.0,
                MaxAnisotropy: 1,
                ComparisonFunc: windows::Win32::Graphics::Direct3D11::D3D11_COMPARISON_NEVER,
                BorderColor: [0.0; 4],
                MinLOD: 0.0,
                MaxLOD: f32::MAX,
            };
            let mut sampler = None;
            device.CreateSamplerState(&sampler_desc, Some(&mut sampler))?;
            let sampler = sampler.ok_or_else(Error::from_win32)?;

            let rasterizer_desc = D3D11_RASTERIZER_DESC {
                FillMode: windows::Win32::Graphics::Direct3D11::D3D11_FILL_SOLID,
                CullMode: windows::Win32::Graphics::Direct3D11::D3D11_CULL_NONE,
                FrontCounterClockwise: BOOL(0),
                DepthBias: 0,
                DepthBiasClamp: 0.0,
                SlopeScaledDepthBias: 0.0,
                DepthClipEnable: BOOL(1),
                ScissorEnable: BOOL(1),
                MultisampleEnable: BOOL(0),
                AntialiasedLineEnable: BOOL(0),
            };
            let mut rasterizer = None;
            device.CreateRasterizerState(&rasterizer_desc, Some(&mut rasterizer))?;
            let rasterizer = rasterizer.ok_or_else(Error::from_win32)?;
            let vertex_shader = create_vertex_shader(&device)?;
            let prepare_pixel_shader = create_pixel_shader(&device, PS_PREPARE)?;
            let horizontal_pixel_shader = create_pixel_shader(&device, PS_BLUR_HORIZONTAL)?;
            let vertical_pixel_shader = create_pixel_shader(&device, PS_BLUR_VERTICAL)?;
            let wide_horizontal_pixel_shader =
                create_pixel_shader(&device, PS_BLUR_WIDE_HORIZONTAL)?;
            let wide_vertical_pixel_shader = create_pixel_shader(&device, PS_BLUR_WIDE_VERTICAL)?;
            let blur_pixel_shader = create_pixel_shader(&device, PS_BLUR)?;
            let composite_pixel_shader = create_pixel_shader(&device, PS_COMPOSITE)?;

            draw_ambient(AmbientPass {
                context: &context,
                render_target: &render_target,
                scene_buffer: &scene_buffer,
                scene: &scene,
                blur_render_target: &blur_render_target,
                blur_view: &blur_view,
                prefilter_render_target: &prefilter_render_target,
                prefilter_view: &prefilter_view,
                horizontal_render_target: &horizontal_render_target,
                horizontal_view: &horizontal_view,
                wide_horizontal_render_target: &wide_horizontal_render_target,
                wide_horizontal_view: &wide_horizontal_view,
                wide_blur_render_target: &wide_blur_render_target,
                wide_blur_view: &wide_blur_view,
                source: &source_view,
                vertex_shader: &vertex_shader,
                prepare_pixel_shader: &prepare_pixel_shader,
                horizontal_pixel_shader: &horizontal_pixel_shader,
                vertical_pixel_shader: &vertical_pixel_shader,
                wide_horizontal_pixel_shader: &wide_horizontal_pixel_shader,
                wide_vertical_pixel_shader: &wide_vertical_pixel_shader,
                blur_pixel_shader: &blur_pixel_shader,
                composite_pixel_shader: &composite_pixel_shader,
                sampler: &sampler,
                rasterizer: &rasterizer,
                width: WIDTH,
                height: HEIGHT,
                blur_width,
                blur_height,
            })?;

            let staging_desc = D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                ..texture_desc
            };
            let mut staging = None;
            device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
            let staging: ID3D11Texture2D = staging.ok_or_else(Error::from_win32)?;
            context.CopyResource(&staging, &output_texture);
            let mut mapped =
                windows::Win32::Graphics::Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;

            let mut result = Vec::with_capacity((WIDTH * HEIGHT) as usize);
            for y in 0..HEIGHT {
                let row = std::slice::from_raw_parts(
                    mapped
                        .pData
                        .cast::<u8>()
                        .add((y * mapped.RowPitch) as usize),
                    (WIDTH * 4) as usize,
                );
                for pixel in 0..WIDTH as usize {
                    let start = pixel * 4;
                    result.push([row[start], row[start + 1], row[start + 2], row[start + 3]]);
                }
            }
            context.Unmap(&staging, 0);
            Ok(result)
        }
    }

    #[test]
    fn shader_blurs_around_all_sides_and_corners_of_an_offset_window() {
        let content = ScreenRect {
            left: 10,
            top: 6,
            right: 30,
            bottom: 18,
        };
        let scene = render_scene(content, [0, 0, 0, 255], [30, 150, 230, 255], false)
            .expect("WARP shader render should succeed for an arbitrary window");
        let alpha = |x: u32, y: u32| scene[(y * WIDTH + x) as usize][3];

        assert!(alpha(20, 1) > 100, "top border should render");
        assert!(alpha(20, 22) > 100, "bottom border should render");
        assert!(alpha(4, 12) > 100, "left border should render");
        assert!(alpha(38, 12) > 100, "right border should render");
        assert!(alpha(4, 1) > 100, "corner should render");
        assert_eq!(alpha(20, 12), 0, "foreground content must remain clear");
    }

    #[test]
    fn shader_softens_window_colors_and_fades_away_without_background_contamination() {
        let content = ScreenRect {
            left: 8,
            top: 4,
            right: 32,
            bottom: 20,
        };
        let scene = render_scene_with_frame(
            content,
            [0, 0, 0, 255],
            [20, 60, 220, 255],
            true,
            TestPattern {
                striped_content: true,
                ..TestPattern::default()
            },
        )
        .expect("WARP should smoothly blur contrasting window colors");
        let pixel = |x: u32, y: u32| scene[(y * WIDTH + x) as usize];

        let near = pixel(20, 3);
        let near_alpha = pixel(7, 12)[3];
        let far_alpha = pixel(0, 12)[3];
        assert!(
            near_alpha > far_alpha,
            "ambient light should fade with distance: {near_alpha} -> {far_alpha}"
        );
        assert_eq!(pixel(20, 12)[3], 0, "foreground content must remain clear");
        assert!(
            near[0] > 20 && near[2] > 20,
            "blur should mix colors from the window: {near:?}"
        );
        assert!(
            near[1] < 100,
            "surrounding green pixels must not contaminate the blur: {near:?}"
        );

        let left = pixel(19, 3);
        let right = pixel(20, 3);
        let adjacent_change = (0..3)
            .map(|channel| left[channel].abs_diff(right[channel]))
            .max()
            .unwrap();
        assert!(
            adjacent_change < 20,
            "neighboring pixels should transition softly: {left:?} -> {right:?}"
        );
    }

    #[test]
    fn reflected_window_color_keeps_a_small_red_detail_local() {
        let content = ScreenRect {
            left: 8,
            top: 4,
            right: 32,
            bottom: 20,
        };
        let scene = render_scene_with_frame(
            content,
            [0, 0, 0, 255],
            [20, 60, 220, 255],
            false,
            TestPattern {
                red_box: Some(ScreenRect {
                    left: 28,
                    top: 9,
                    right: 31,
                    bottom: 13,
                }),
                ..TestPattern::default()
            },
        )
        .expect("WARP should reflect local window details into the ambient border");
        let pixel = |x: u32, y: u32| scene[(y * WIDTH + x) as usize];
        let near_detail = pixel(33, 9);
        let away_from_detail = pixel(6, 9);

        assert!(
            near_detail[0] > away_from_detail[0],
            "a nearby reflected edge should retain slightly more of the red detail: {near_detail:?} vs {away_from_detail:?}"
        );
        assert_eq!(pixel(20, 12)[3], 0, "foreground content must remain clear");
    }

    #[test]
    fn reflected_window_edges_follow_local_colors_and_blend_corners() {
        let content = ScreenRect {
            left: 8,
            top: 4,
            right: 32,
            bottom: 20,
        };
        let scene = render_scene_with_frame(
            content,
            [0, 0, 0, 255],
            [40, 40, 40, 255],
            false,
            TestPattern {
                spatial_gradient: true,
                ..TestPattern::default()
            },
        )
        .expect("WARP should reflect distinct window edges and corners");
        let pixel = |x: u32, y: u32| scene[(y * WIDTH + x) as usize];
        let left = pixel(4, 12);
        let right = pixel(38, 12);
        let top = pixel(20, 1);
        let bottom = pixel(20, 22);
        let corner = pixel(38, 1);

        assert!(
            right[0] > left[0] + 20,
            "right edge should reflect its redder content: {right:?} vs {left:?}"
        );
        assert!(
            top[1] > bottom[1] + 20,
            "top edge should reflect its greener content: {top:?} vs {bottom:?}"
        );
        assert!(
            corner[0] > top[0],
            "corner blend should include the right edge: {corner:?} vs {top:?}"
        );
        assert!(
            corner[1] > right[1],
            "corner blend should include the top edge: {corner:?} vs {right:?}"
        );
    }

    #[test]
    fn shader_samples_beyond_a_dark_window_frame_on_all_sides() {
        let content = ScreenRect {
            left: 8,
            top: 4,
            right: 32,
            bottom: 20,
        };
        let scene = render_scene_with_frame(
            content,
            [0, 0, 0, 255],
            [20, 150, 230, 255],
            false,
            TestPattern {
                frame: Some(([3, 3, 3, 255], 3)),
                ..TestPattern::default()
            },
        )
        .expect("WARP should render ambient color from the window interior");
        let pixel = |x: u32, y: u32| scene[(y * WIDTH + x) as usize];

        for (x, y, side) in [
            (20, 1, "top"),
            (20, 22, "bottom"),
            (4, 12, "left"),
            (38, 12, "right"),
            (4, 1, "corner"),
        ] {
            let lit = pixel(x, y);
            assert!(
                lit[1] > 20 || lit[2] > 20,
                "{side} should carry interior color: {lit:?}"
            );
            assert!(lit[3] > 100, "{side} should be visible: {lit:?}");
        }
        assert_eq!(pixel(20, 12)[3], 0, "foreground content must remain clear");
    }

    #[test]
    fn fixed_content_stays_clear_and_margins_render_immediately_with_black_content() {
        let content = ScreenRect {
            left: 8,
            top: 4,
            right: 32,
            bottom: 20,
        };
        let scene = render_scene(content, [0, 0, 0, 255], [0, 0, 0, 255], false)
            .expect("WARP should render immediately without pixel-based bar detection");
        let alpha = |x: u32, y: u32| scene[(y * WIDTH + x) as usize][3];
        assert!(
            alpha(1, 12) > 100,
            "left margin should render despite black content"
        );
        assert!(
            alpha(20, 0) > 100,
            "top margin should render despite black content"
        );
        assert!(
            alpha(20, 23) > 100,
            "bottom margin should render despite black content"
        );
        assert!(
            alpha(1, 1) > 100,
            "corner should render despite black content"
        );
        assert_eq!(alpha(20, 12), 0, "content must remain clear");
    }

    #[test]
    fn bright_margin_pixels_do_not_change_reflected_ambient_colors() {
        let content = ScreenRect {
            left: 8,
            top: 4,
            right: 32,
            bottom: 20,
        };
        let plain = render_scene(content, [0, 0, 0, 255], [30, 150, 230, 255], false)
            .expect("WARP should render the baseline fixed margin");
        let bright = render_scene(content, [0, 0, 0, 255], [30, 150, 230, 255], true)
            .expect("WARP should render the same fixed margin with a bright outside pixel");
        let bright_margins =
            render_scene(content, [255, 255, 255, 255], [30, 150, 230, 255], false)
                .expect("WARP should render immediately even with bright margins");

        let sample = (HEIGHT / 2 * WIDTH + 2) as usize;
        assert_eq!(bright[sample], plain[sample]);
        assert_eq!(bright_margins[sample], plain[sample]);
        assert!(
            bright[sample][3] > 100,
            "bright margin pixels must not disable the configured effect"
        );
    }

    #[test]
    fn capture_retry_backoff_is_capped_and_resets_after_recovery() {
        let mut retry = CaptureRetry::default();
        assert_eq!(retry.delay, CAPTURE_RETRY_INITIAL);

        retry.schedule();
        assert_eq!(retry.delay, Duration::from_millis(500));
        retry.schedule();
        assert_eq!(retry.delay, Duration::from_secs(1));
        retry.schedule();
        assert_eq!(retry.delay, Duration::from_secs(2));
        retry.schedule();
        assert_eq!(retry.delay, CAPTURE_RETRY_MAX);
        retry.schedule();
        assert_eq!(retry.delay, CAPTURE_RETRY_MAX);

        retry.reset();
        assert!(retry.is_ready());
        assert_eq!(retry.delay, CAPTURE_RETRY_INITIAL);
    }

    #[test]
    fn initial_capture_waits_briefly_but_cached_capture_is_nonblocking() {
        assert_eq!(capture_timeout_ms(false), INITIAL_FRAME_TIMEOUT_MS);
        assert_eq!(capture_timeout_ms(true), 0);
    }
}
