#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContentRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AspectRatio {
    pub width: u32,
    pub height: u32,
}

impl AspectRatio {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        if width == 0
            || height == 0
            || u64::from(width) * 4 < u64::from(height)
            || u64::from(width) > u64::from(height) * 4
        {
            return None;
        }
        Some(Self { width, height })
    }
}

impl ScreenRect {
    pub fn is_empty(self) -> bool {
        self.left >= self.right || self.top >= self.bottom
    }
}

pub fn fit_content(width: u32, height: u32, ratio: AspectRatio) -> Option<ScreenRect> {
    let (screen_width, screen_height) = (i32::try_from(width).ok()?, i32::try_from(height).ok()?);
    if width == 0 || height == 0 {
        return None;
    }

    let (content_width, content_height) = if u64::from(width) * u64::from(ratio.height)
        >= u64::from(height) * u64::from(ratio.width)
    {
        (
            u32::try_from(
                (u64::from(height) * u64::from(ratio.width)).div_ceil(u64::from(ratio.height)),
            )
            .ok()?,
            height,
        )
    } else {
        (
            width,
            u32::try_from(
                (u64::from(width) * u64::from(ratio.height)).div_ceil(u64::from(ratio.width)),
            )
            .ok()?,
        )
    };
    let left = (width - content_width) / 2;
    let top = (height - content_height) / 2;
    Some(ScreenRect {
        left: i32::try_from(left).ok()?,
        top: i32::try_from(top).ok()?,
        right: i32::try_from(left + content_width).ok()?.min(screen_width),
        bottom: i32::try_from(top + content_height).ok()?.min(screen_height),
    })
}

pub fn clip_to_screen(rect: ScreenRect, width: u32, height: u32) -> Option<ScreenRect> {
    if width == 0 || height == 0 {
        return None;
    }

    let bounds = ScreenRect {
        left: rect.left.max(0),
        top: rect.top.max(0),
        right: rect.right.min(width as i32),
        bottom: rect.bottom.min(height as i32),
    };
    (!bounds.is_empty()).then_some(bounds)
}

pub fn content_bounds(rect: ScreenRect, width: u32, height: u32) -> Option<ContentRect> {
    let clipped = clip_to_screen(rect, width, height)?;
    Some(ContentRect {
        left: clipped.left as f32 / width as f32,
        top: clipped.top as f32 / height as f32,
        right: clipped.right as f32 / width as f32,
        bottom: clipped.bottom as f32 / height as f32,
    })
}

pub fn ambient_regions(rect: ScreenRect, width: u32, height: u32) -> [ScreenRect; 4] {
    let Some(content) = clip_to_screen(rect, width, height) else {
        return [ScreenRect::default(); 4];
    };
    let bounds = ScreenRect {
        left: 0,
        top: 0,
        right: width as i32,
        bottom: height as i32,
    };
    [
        ScreenRect {
            left: bounds.left,
            top: bounds.top,
            right: bounds.right,
            bottom: content.top,
        },
        ScreenRect {
            left: bounds.left,
            top: content.bottom,
            right: bounds.right,
            bottom: bounds.bottom,
        },
        ScreenRect {
            left: bounds.left,
            top: content.top,
            right: content.left,
            bottom: content.bottom,
        },
        ScreenRect {
            left: content.right,
            top: content.top,
            right: bounds.right,
            bottom: content.bottom,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_centered_16_9_content_on_ultrawide_screen() {
        assert_eq!(
            fit_content(3440, 1440, AspectRatio::new(16, 9).unwrap()),
            Some(ScreenRect {
                left: 440,
                top: 0,
                right: 3000,
                bottom: 1440,
            })
        );
    }

    #[test]
    fn uses_the_entire_16_9_screen() {
        assert_eq!(
            fit_content(1920, 1080, AspectRatio::new(16, 9).unwrap()),
            Some(ScreenRect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            })
        );
    }

    #[test]
    fn fits_portrait_screen_inside_the_centered_content_rectangle() {
        assert_eq!(
            fit_content(1080, 1920, AspectRatio::new(16, 9).unwrap()),
            Some(ScreenRect {
                left: 0,
                top: 656,
                right: 1080,
                bottom: 1264,
            })
        );
    }

    #[test]
    fn rounds_odd_screen_margins_without_clipping_the_content_rectangle() {
        assert_eq!(
            fit_content(3441, 1440, AspectRatio::new(16, 9).unwrap()),
            Some(ScreenRect {
                left: 440,
                top: 0,
                right: 3000,
                bottom: 1440,
            })
        );
        assert_eq!(fit_content(0, 1080, AspectRatio::new(16, 9).unwrap()), None);
    }

    #[test]
    fn fits_custom_aspect_ratios_and_rejects_invalid_ratios() {
        assert_eq!(
            fit_content(3440, 1440, AspectRatio::new(21, 9).unwrap()),
            Some(ScreenRect {
                left: 40,
                top: 0,
                right: 3400,
                bottom: 1440,
            })
        );
        assert_eq!(
            fit_content(3440, 1440, AspectRatio::new(4, 3).unwrap()),
            Some(ScreenRect {
                left: 760,
                top: 0,
                right: 2680,
                bottom: 1440,
            })
        );
        assert_eq!(
            fit_content(1920, 1080, AspectRatio::new(9, 16).unwrap()),
            Some(ScreenRect {
                left: 656,
                top: 0,
                right: 1264,
                bottom: 1080,
            })
        );
        assert_eq!(AspectRatio::new(0, 9), None);
        assert_eq!(AspectRatio::new(16, 0), None);
        assert_eq!(AspectRatio::new(5, 1), None);
    }

    #[test]
    fn clips_window_bounds_to_the_captured_screen() {
        let rect = content_bounds(
            ScreenRect {
                left: -100,
                top: 120,
                right: 900,
                bottom: 900,
            },
            800,
            600,
        )
        .unwrap();

        assert_eq!(
            rect,
            ContentRect {
                left: 0.0,
                top: 0.2,
                right: 1.0,
                bottom: 1.0
            }
        );
    }

    #[test]
    fn rejects_windows_that_do_not_intersect_the_screen() {
        assert!(
            content_bounds(
                ScreenRect {
                    left: 900,
                    top: 10,
                    right: 1000,
                    bottom: 500,
                },
                800,
                600,
            )
            .is_none()
        );
    }

    #[test]
    fn creates_four_nonoverlapping_ambient_regions_for_an_offset_window() {
        let regions = ambient_regions(
            ScreenRect {
                left: 100,
                top: 80,
                right: 700,
                bottom: 500,
            },
            800,
            600,
        );

        assert_eq!(
            regions[0],
            ScreenRect {
                left: 0,
                top: 0,
                right: 800,
                bottom: 80
            }
        );
        assert_eq!(
            regions[1],
            ScreenRect {
                left: 0,
                top: 500,
                right: 800,
                bottom: 600
            }
        );
        assert_eq!(
            regions[2],
            ScreenRect {
                left: 0,
                top: 80,
                right: 100,
                bottom: 500
            }
        );
        assert_eq!(
            regions[3],
            ScreenRect {
                left: 700,
                top: 80,
                right: 800,
                bottom: 500
            }
        );
    }
}
