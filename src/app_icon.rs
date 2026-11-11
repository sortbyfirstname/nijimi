use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;

use gpui_kit::RenderImage;
use image::{Frame, RgbaImage};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, GdiFlush, HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::{
    DI_NORMAL, DestroyIcon, DrawIconEx, HICON, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR,
    LR_SHARED, LoadImageW, SendMessageW, WM_SETICON,
};
use windows::core::{PCWSTR, Result};

const ICON_SIZE: i32 = 32;
const APPLICATION_ICON_RESOURCE_ID: usize = 1;

pub fn load_application_icon(size: i32) -> Result<HICON> {
    unsafe {
        let module = GetModuleHandleW(None)?;
        let icon = LoadImageW(
            module,
            PCWSTR(APPLICATION_ICON_RESOURCE_ID as *const u16),
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR | LR_SHARED,
        )?;
        Ok(HICON(icon.0))
    }
}

pub fn set_window_icons(hwnd: windows::Win32::Foundation::HWND) -> Result<()> {
    let small = load_application_icon(16)?;
    let large = load_application_icon(32)?;
    unsafe {
        let _ = SendMessageW(
            hwnd,
            WM_SETICON,
            windows::Win32::Foundation::WPARAM(ICON_SMALL as usize),
            windows::Win32::Foundation::LPARAM(small.0 as isize),
        );
        let _ = SendMessageW(
            hwnd,
            WM_SETICON,
            windows::Win32::Foundation::WPARAM(ICON_BIG as usize),
            windows::Win32::Foundation::LPARAM(large.0 as isize),
        );
    }
    Ok(())
}

struct OwnedIcon(HICON);

impl Drop for OwnedIcon {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyIcon(self.0);
        }
    }
}

struct IconSurface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
}

impl IconSurface {
    fn new() -> Option<Self> {
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.0.is_null() {
                return None;
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: ICON_SIZE,
                    biHeight: -ICON_SIZE,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bitmap) => bitmap,
                Err(_) => {
                    let _ = DeleteDC(dc);
                    return None;
                }
            };
            if bits.is_null() {
                let _ = DeleteObject(bitmap);
                let _ = DeleteDC(dc);
                return None;
            }
            let previous = SelectObject(dc, bitmap);
            if previous.0.is_null() || previous.0 as isize == -1 {
                let _ = DeleteObject(bitmap);
                let _ = DeleteDC(dc);
                return None;
            }
            Some(Self {
                dc,
                bitmap,
                previous,
                bits: bits.cast(),
            })
        }
    }

    fn draw(&mut self, icon: HICON, background: u8) -> Option<Vec<u8>> {
        let len = (ICON_SIZE * ICON_SIZE * 4) as usize;
        unsafe {
            std::ptr::write_bytes(self.bits, background, len);
            DrawIconEx(
                self.dc, 0, 0, icon, ICON_SIZE, ICON_SIZE, 0, None, DI_NORMAL,
            )
            .ok()?;
            GdiFlush().ok().ok()?;
            Some(std::slice::from_raw_parts(self.bits, len).to_vec())
        }
    }
}

impl Drop for IconSurface {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.dc, self.previous);
            let _ = DeleteDC(self.dc);
            let _ = DeleteObject(self.bitmap);
        }
    }
}

fn straight_alpha(black: &[u8], white: &[u8]) -> Vec<u8> {
    // The background difference reveals transparency for both alpha and mask icons.
    // GPUI expects straight-alpha BGRA, not GDI's composited pixel values.
    let mut result = Vec::with_capacity(black.len());
    for (black, white) in black
        .as_chunks::<4>()
        .0
        .iter()
        .zip(white.as_chunks::<4>().0)
    {
        let difference: u32 = (0..3)
            .map(|channel| u32::from(white[channel].saturating_sub(black[channel])))
            .sum();
        let alpha = 255 - ((difference + 1) / 3);
        for &channel in &black[..3] {
            let value = (u32::from(channel) * 255 + alpha / 2)
                .checked_div(alpha)
                .unwrap_or(0)
                .min(255);
            result.push(value as u8);
        }
        result.push(alpha as u8);
    }
    result
}

pub fn executable_icon(path: &Path) -> Option<Arc<RenderImage>> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.is_empty() || wide.contains(&0) || !path.is_file() {
        return None;
    }
    wide.push(0);

    unsafe {
        let mut handle = HICON::default();
        let extracted = ExtractIconExW(PCWSTR(wide.as_ptr()), 0, Some(&mut handle), None, 1);
        if handle.0.is_null() {
            return None;
        }
        let icon = OwnedIcon(handle);
        if extracted == 0 {
            return None;
        }
        let mut surface = IconSurface::new()?;
        let black = surface.draw(icon.0, 0)?;
        let white = surface.draw(icon.0, 255)?;
        let pixels = straight_alpha(&black, &white);
        let image = RgbaImage::from_raw(ICON_SIZE as u32, ICON_SIZE as u32, pixels)?;
        Some(Arc::new(RenderImage::new(vec![Frame::new(image)])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;
    use windows::Win32::Graphics::Gdi::CreateBitmap;
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};

    #[test]
    fn reconstructs_straight_alpha_bgra() {
        let black = [0, 0, 0, 0, 10, 20, 30, 0, 0, 64, 128, 0];
        let white = [255, 255, 255, 0, 10, 20, 30, 0, 127, 191, 255, 0];
        assert_eq!(
            straight_alpha(&black, &white),
            [0, 0, 0, 0, 10, 20, 30, 255, 0, 128, 255, 128]
        );
    }

    #[test]
    fn reconstructs_real_alpha_icon_drawn_by_gdi() {
        unsafe {
            let color = IconSurface::new().unwrap();
            for pixel in
                std::slice::from_raw_parts_mut(color.bits, (ICON_SIZE * ICON_SIZE * 4) as usize)
                    .as_chunks_mut::<4>()
                    .0
            {
                pixel.copy_from_slice(&[0, 128, 255, 128]);
            }
            let mask = CreateBitmap(ICON_SIZE, ICON_SIZE, 1, 1, None);
            assert!(!mask.0.is_null());
            let info = ICONINFO {
                fIcon: true.into(),
                hbmMask: mask,
                hbmColor: color.bitmap,
                ..Default::default()
            };
            let icon = CreateIconIndirect(&info);
            let _ = DeleteObject(mask);
            let icon = OwnedIcon(icon.unwrap());
            let mut surface = IconSurface::new().unwrap();
            let black = surface.draw(icon.0, 0).unwrap();
            let white = surface.draw(icon.0, 255).unwrap();
            let pixels = straight_alpha(&black, &white);
            let pixel = &pixels[..4];
            assert!(pixel[0] <= 2, "{pixel:?}");
            assert!((126..=130).contains(&pixel[1]), "{pixel:?}");
            assert!(pixel[2] >= 253, "{pixel:?}");
            assert!((126..=130).contains(&pixel[3]), "{pixel:?}");
        }
    }

    #[test]
    fn reconstructs_legacy_mask_icon() {
        unsafe {
            let color = IconSurface::new().unwrap();
            for (index, pixel) in
                std::slice::from_raw_parts_mut(color.bits, (ICON_SIZE * ICON_SIZE * 4) as usize)
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .enumerate()
            {
                if index % (ICON_SIZE as usize) < ICON_SIZE as usize / 2 {
                    pixel.copy_from_slice(&[10, 20, 30, 0]);
                } else {
                    pixel.fill(0);
                }
            }
            let mut mask_bits = [0u8; (ICON_SIZE * ICON_SIZE / 8) as usize];
            for row in mask_bits.as_chunks_mut::<4>().0 {
                row[2..].fill(255);
            }
            let mask = CreateBitmap(ICON_SIZE, ICON_SIZE, 1, 1, Some(mask_bits.as_ptr().cast()));
            assert!(!mask.0.is_null());
            let info = ICONINFO {
                fIcon: true.into(),
                hbmMask: mask,
                hbmColor: color.bitmap,
                ..Default::default()
            };
            let icon = CreateIconIndirect(&info);
            let _ = DeleteObject(mask);
            let icon = OwnedIcon(icon.unwrap());
            let mut surface = IconSurface::new().unwrap();
            let black = surface.draw(icon.0, 0).unwrap();
            let white = surface.draw(icon.0, 255).unwrap();
            let pixels = straight_alpha(&black, &white);
            assert_eq!(&pixels[..4], &[10, 20, 30, 255]);
            assert_eq!(&pixels[31 * 4..32 * 4], &[0, 0, 0, 0]);
        }
    }

    #[test]
    fn extracts_system_executable_icon() {
        let path = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("notepad.exe");
        let icon = executable_icon(&path).expect("system executable icon");
        assert_eq!(icon.frame_count(), 1);
        let pixels = icon.as_bytes(0).unwrap();
        assert_eq!(pixels.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        assert!(pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0));
    }

    #[test]
    fn embedded_application_icon_loads_for_shell_sizes_and_extracts_from_executable() {
        for size in [16, 24, 32, 48, 64, 128, 256] {
            let icon = load_application_icon(size).expect("embedded application icon");
            let mut surface = IconSurface::new().expect("icon drawing surface");
            let pixels = surface.draw(icon, 0).expect("draw the embedded icon");
            assert!(
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[..3] != [0, 0, 0])
            );
            if size == 64 {
                assert!(
                    pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .any(|pixel| { pixel[0] != pixel[1] || pixel[1] != pixel[2] }),
                    "the embedded logo should retain its original colors"
                );
            }
        }

        let executable = std::env::current_exe().expect("test executable path");
        let icon = executable_icon(&executable).expect("Shell should extract the embedded icon");
        assert!(
            icon.as_bytes(0)
                .unwrap()
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] != 0)
        );
    }

    #[test]
    fn rejects_missing_directory_and_nul_paths() {
        assert!(executable_icon(Path::new("")).is_none());
        assert!(executable_icon(&std::env::temp_dir()).is_none());
        assert!(executable_icon(&std::env::temp_dir().join("nijimi-no-such-icon.exe")).is_none());
        assert!(executable_icon(&std::env::current_dir().unwrap().join("Cargo.toml")).is_none());
        let nul = PathBuf::from(OsString::from_wide(&[b'a' as u16, 0, b'b' as u16]));
        assert!(executable_icon(&nul).is_none());
    }
}
