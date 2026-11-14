//! Renders the ambient effect around a sample image, for the Settings window.

use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_RESOURCE_MISC_GENERATE_MIPS, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};

use super::*;

const SAMPLE_JPEG: &[u8] = include_bytes!("../../assets/preview-sample.jpg");
const PS_PREVIEW_SCREEN: &[u8] = shader_bytecode!("PSPreviewScreen");
const PS_PREVIEW_FINAL: &[u8] = shader_bytecode!("PSPreviewFinal");
/// The preview renders a screen this many times smaller than the real one, with the blur
/// downsampled that much less, so the blur itself runs on exactly as many texels as on screen.
const SHRINK: u32 = 2;
/// The returned image is the first mip level no wider than this.
const MAX_IMAGE_WIDTH: u32 = 1600;

pub struct PreviewImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

pub struct Preview {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    shaders: AmbientShaders,
    screen_shader: ID3D11PixelShader,
    final_shader: ID3D11PixelShader,
    scene_buffer: ID3D11Buffer,
    sample: ID3D11ShaderResourceView,
    targets: Option<PreviewTargets>,
}

// Direct3D 11 devices are free-threaded; the immediate context only needs one user at a time,
// which owning the `Preview` guarantees.
unsafe impl Send for Preview {}

struct PreviewTargets {
    width: u32,
    height: u32,
    blur: BlurTargets,
    screen: BlurTarget,
    overlay: BlurTarget,
    output: ID3D11Texture2D,
    output_target: ID3D11RenderTargetView,
    output_view: ID3D11ShaderResourceView,
    level: u32,
    level_width: u32,
    level_height: u32,
    staging: ID3D11Texture2D,
}

/// The primary display's resolution in physical pixels, regardless of DPI awareness.
pub fn primary_screen_size() -> Option<(u32, u32)> {
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    unsafe { EnumDisplaySettingsW(None, ENUM_CURRENT_SETTINGS, &mut mode) }
        .as_bool()
        .then_some((mode.dmPelsWidth, mode.dmPelsHeight))
        .filter(|&(width, height)| width > 0 && height > 0)
}

impl Preview {
    pub fn new() -> Result<Self> {
        let sample = image::load_from_memory_with_format(SAMPLE_JPEG, image::ImageFormat::Jpeg)
            .map_err(|error| Error::new(windows::Win32::Foundation::E_FAIL, error.to_string()))?
            .to_rgba8();
        let (device, context) = create_device(None, D3D_DRIVER_TYPE_HARDWARE)
            .or_else(|_| create_device(None, D3D_DRIVER_TYPE_WARP))?;
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: sample.width(),
                Height: sample.height(),
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
            let data = D3D11_SUBRESOURCE_DATA {
                pSysMem: sample.as_ptr().cast(),
                SysMemPitch: sample.width() * 4,
                SysMemSlicePitch: 0,
            };
            let mut texture = None;
            device.CreateTexture2D(&desc, Some(&data), Some(&mut texture))?;
            let texture = texture.ok_or_else(Error::from_win32)?;
            let mut view = None;
            device.CreateShaderResourceView(&texture, None, Some(&mut view))?;
            Ok(Self {
                shaders: AmbientShaders::new(&device)?,
                screen_shader: create_pixel_shader(&device, PS_PREVIEW_SCREEN)?,
                final_shader: create_pixel_shader(&device, PS_PREVIEW_FINAL)?,
                scene_buffer: create_scene_buffer(&device)?,
                sample: view.ok_or_else(Error::from_win32)?,
                targets: None,
                device,
                context,
            })
        }
    }

    pub fn render(
        &mut self,
        screen_width: u32,
        screen_height: u32,
        content_area: ContentArea,
        effect: EffectSettings,
    ) -> Result<PreviewImage> {
        let width = (screen_width / SHRINK).max(1);
        let height = (screen_height / SHRINK).max(1);
        let content_area = match content_area {
            ContentArea::Size { width, height } => ContentArea::Size {
                width: (width / SHRINK).max(1),
                height: (height / SHRINK).max(1),
            },
            ratio => ratio,
        };
        let screen_scale = BlurScale::new(effect.resolution.scale());
        let blur_scale = BlurScale {
            prefilter: (screen_scale.prefilter / SHRINK).max(1),
            glow: (screen_scale.glow / SHRINK).max(1),
        };
        let scene = SceneGeometry::with_blur_scale(width, height, content_area, effect, blur_scale)
            .ok_or_else(Error::from_win32)?;
        if self
            .targets
            .as_ref()
            .is_none_or(|targets| targets.width != width || targets.height != height)
        {
            self.targets = Some(PreviewTargets::new(
                &self.device,
                width,
                height,
                blur_scale,
            )?);
        }
        let targets = self.targets.as_mut().unwrap();
        if targets.blur.scale != blur_scale {
            targets.blur = BlurTargets::new(&self.device, width, height, blur_scale)?;
        }

        unsafe {
            let context = &self.context;
            context.UpdateSubresource(
                &self.scene_buffer,
                0,
                None,
                &scene.constants as *const SceneConstants as *const std::ffi::c_void,
                0,
                0,
            );
            let pipeline = FullscreenPipeline {
                context,
                vertex_shader: &self.shaders.vertex,
                scene_buffer: &self.scene_buffer,
                sampler: &self.shaders.sampler,
                rasterizer: &self.shaders.rasterizer,
            };
            let whole = [RECT {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            }];
            draw_fullscreen_pass(
                &pipeline,
                FullscreenPass {
                    render_target: &targets.screen.render_target,
                    width,
                    height,
                    pixel_shader: &self.screen_shader,
                    sources: [
                        Some(targets.overlay.view.clone()),
                        Some(self.sample.clone()),
                        None,
                        None,
                        None,
                    ],
                    scissors: &whole,
                },
            );
            draw_ambient(AmbientPass {
                context,
                render_target: &targets.overlay.render_target,
                scene_buffer: &self.scene_buffer,
                scene: &scene,
                blur: &targets.blur,
                source: &targets.screen.view,
                shaders: &self.shaders,
                width,
                height,
            })?;
            draw_fullscreen_pass(
                &pipeline,
                FullscreenPass {
                    render_target: &targets.output_target,
                    width,
                    height,
                    pixel_shader: &self.final_shader,
                    sources: [
                        Some(targets.screen.view.clone()),
                        Some(targets.overlay.view.clone()),
                        None,
                        None,
                        None,
                    ],
                    scissors: &whole,
                },
            );
            context.OMSetRenderTargets(None, None::<&ID3D11DepthStencilView>);
            context.GenerateMips(&targets.output_view);
            context.CopySubresourceRegion(
                &targets.staging,
                0,
                0,
                0,
                0,
                &targets.output,
                targets.level,
                None,
            );
            targets.read_staging(context)
        }
    }
}

impl PreviewTargets {
    fn new(device: &ID3D11Device, width: u32, height: u32, blur_scale: BlurScale) -> Result<Self> {
        let screen_desc = D3D11_TEXTURE2D_DESC {
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
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32 | D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let level = (0..)
            .find(|level| (width >> level).max(1) <= MAX_IMAGE_WIDTH)
            .unwrap();
        let level_width = (width >> level).max(1);
        let level_height = (height >> level).max(1);
        unsafe {
            let mut output = None;
            device.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    MipLevels: level + 1,
                    MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
                    ..screen_desc
                },
                None,
                Some(&mut output),
            )?;
            let output: ID3D11Texture2D = output.ok_or_else(Error::from_win32)?;
            let mut output_target = None;
            device.CreateRenderTargetView(&output, None, Some(&mut output_target))?;
            let mut output_view = None;
            device.CreateShaderResourceView(&output, None, Some(&mut output_view))?;
            let mut staging = None;
            device.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: level_width,
                    Height: level_height,
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    ..screen_desc
                },
                None,
                Some(&mut staging),
            )?;
            Ok(Self {
                width,
                height,
                blur: BlurTargets::new(device, width, height, blur_scale)?,
                screen: create_blur_target(device, &screen_desc)?,
                overlay: create_blur_target(device, &screen_desc)?,
                output,
                output_target: output_target.ok_or_else(Error::from_win32)?,
                output_view: output_view.ok_or_else(Error::from_win32)?,
                level,
                level_width,
                level_height,
                staging: staging.ok_or_else(Error::from_win32)?,
            })
        }
    }

    unsafe fn read_staging(&self, context: &ID3D11DeviceContext) -> Result<PreviewImage> {
        unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let row_bytes = (self.level_width * 4) as usize;
            let mut bgra = Vec::with_capacity(row_bytes * self.level_height as usize);
            for y in 0..self.level_height as usize {
                let row = std::slice::from_raw_parts(
                    mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                    row_bytes,
                );
                bgra.extend_from_slice(row);
            }
            context.Unmap(&self.staging, 0);
            Ok(PreviewImage {
                width: self.level_width,
                height: self.level_height,
                bgra,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::BlurResolution;

    #[test]
    fn preview_shows_the_sample_inside_and_its_glow_in_the_bars() {
        let mut preview = Preview::new().expect("the preview should render on any device");
        let effect = EffectSettings {
            edge_softness: 0,
            edge_blend: 0,
            blur: 50,
            resolution: BlurResolution::Quarter,
            ..EffectSettings::default()
        };
        let image = preview
            .render(3440, 1440, ContentArea::ratio(16, 9).unwrap(), effect)
            .unwrap();
        assert_eq!((image.width, image.height), (860, 360));
        let pixel = |x: u32, y: u32| {
            let start = ((y * image.width + x) * 4) as usize;
            [
                image.bgra[start],
                image.bgra[start + 1],
                image.bgra[start + 2],
            ]
        };
        let brightness = |[b, g, r]: [u8; 3]| b as u32 + g as u32 + r as u32;

        assert!(
            brightness(pixel(75, 180)) > 60,
            "left bar should glow: {:?}",
            pixel(75, 180)
        );
        assert!(
            brightness(pixel(785, 180)) > 60,
            "right bar should glow: {:?}",
            pixel(785, 180)
        );
        assert!(
            brightness(pixel(430, 180)) > 60,
            "sample should fill the content: {:?}",
            pixel(430, 180)
        );
        assert!(
            brightness(pixel(1, 180)) < brightness(pixel(105, 180)),
            "glow should fade toward the screen edge"
        );
    }
}
