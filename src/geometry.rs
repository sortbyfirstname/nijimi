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

/// The part of the screen the game or other content shows in: a ratio fitted to the screen, or an
/// exact size centred on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentArea {
    Ratio(AspectRatio),
    Size { width: u32, height: u32 },
}

impl ContentArea {
    pub fn ratio(width: u32, height: u32) -> Option<Self> {
        AspectRatio::new(width, height).map(Self::Ratio)
    }

    /// Bounded like a ratio, so a size too big for the screen can fall back to fitting it.
    pub fn size(width: u32, height: u32) -> Option<Self> {
        AspectRatio::new(width, height)?;
        Some(Self::Size { width, height })
    }

    /// `21:9` for a ratio, `2560x1080` for a size.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some((width, height)) = value.split_once(':') {
            return Self::ratio(width.trim().parse().ok()?, height.trim().parse().ok()?);
        }
        let (width, height) = value.split_once(['x', 'X', '×'])?;
        Self::size(width.trim().parse().ok()?, height.trim().parse().ok()?)
    }
}

impl std::fmt::Display for ContentArea {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ratio(ratio) => write!(f, "{}:{}", ratio.width, ratio.height),
            Self::Size { width, height } => write!(f, "{width}x{height}"),
        }
    }
}

impl ScreenRect {
    pub fn is_empty(self) -> bool {
        self.left >= self.right || self.top >= self.bottom
    }
}

pub fn fit_content(width: u32, height: u32, area: ContentArea) -> Option<ScreenRect> {
    match area {
        ContentArea::Ratio(ratio) => fit_ratio(width, height, ratio),
        ContentArea::Size {
            width: content_width,
            height: content_height,
        } if content_width <= width && content_height <= height => {
            let left = i32::try_from((width - content_width) / 2).ok()?;
            let top = i32::try_from((height - content_height) / 2).ok()?;
            Some(ScreenRect {
                left,
                top,
                right: left + i32::try_from(content_width).ok()?,
                bottom: top + i32::try_from(content_height).ok()?,
            })
        }
        ContentArea::Size {
            width: content_width,
            height: content_height,
        } => fit_ratio(
            width,
            height,
            AspectRatio::new(content_width, content_height)?,
        ),
    }
}

/// Like [`fit_content`], but an exact size sits on `window` (the foreground window's client
/// area) when the window is exactly that size and on screen.
pub fn place_content(
    width: u32,
    height: u32,
    area: ContentArea,
    window: Option<ScreenRect>,
) -> Option<ScreenRect> {
    if let ContentArea::Size {
        width: content_width,
        height: content_height,
    } = area
        && let Some(window) = window
        && i64::from(window.right) - i64::from(window.left) == i64::from(content_width)
        && i64::from(window.bottom) - i64::from(window.top) == i64::from(content_height)
        && clip_to_screen(window, width, height).is_some()
    {
        return Some(window);
    }
    fit_content(width, height, area)
}

fn fit_ratio(width: u32, height: u32, ratio: AspectRatio) -> Option<ScreenRect> {
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

/// Shrinks `rect` by `amount` on each side that borders a bar. Sides touching the screen edge
/// stay put, and the inset is capped at a quarter of the content's shorter side.
pub fn inset_bar_edges(rect: ScreenRect, width: u32, height: u32, amount: u32) -> ScreenRect {
    let Some(content) = clip_to_screen(rect, width, height) else {
        return rect;
    };
    let shorter_side = (content.right - content.left).min(content.bottom - content.top);
    let amount = (amount as i32).min(shorter_side / 4);
    let inset = |has_bar: bool| if has_bar { amount } else { 0 };
    ScreenRect {
        left: content.left + inset(content.left > 0),
        top: content.top + inset(content.top > 0),
        right: content.right - inset(content.right < width as i32),
        bottom: content.bottom - inset(content.bottom < height as i32),
    }
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
            fit_content(3440, 1440, ContentArea::ratio(16, 9).unwrap()),
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
            fit_content(1920, 1080, ContentArea::ratio(16, 9).unwrap()),
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
            fit_content(1080, 1920, ContentArea::ratio(16, 9).unwrap()),
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
            fit_content(3441, 1440, ContentArea::ratio(16, 9).unwrap()),
            Some(ScreenRect {
                left: 440,
                top: 0,
                right: 3000,
                bottom: 1440,
            })
        );
        assert_eq!(
            fit_content(0, 1080, ContentArea::ratio(16, 9).unwrap()),
            None
        );
    }

    #[test]
    fn fits_custom_aspect_ratios_and_rejects_invalid_ratios() {
        assert_eq!(
            fit_content(3440, 1440, ContentArea::ratio(21, 9).unwrap()),
            Some(ScreenRect {
                left: 40,
                top: 0,
                right: 3400,
                bottom: 1440,
            })
        );
        assert_eq!(
            fit_content(3440, 1440, ContentArea::ratio(4, 3).unwrap()),
            Some(ScreenRect {
                left: 760,
                top: 0,
                right: 2680,
                bottom: 1440,
            })
        );
        assert_eq!(
            fit_content(1920, 1080, ContentArea::ratio(9, 16).unwrap()),
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
    fn centres_an_exact_size_with_bars_on_every_side() {
        assert_eq!(
            fit_content(2560, 1440, ContentArea::size(1920, 1080).unwrap()),
            Some(ScreenRect {
                left: 320,
                top: 180,
                right: 2240,
                bottom: 1260,
            })
        );
        assert_eq!(
            fit_content(2560, 1440, ContentArea::size(2560, 1440).unwrap()),
            fit_content(2560, 1440, ContentArea::ratio(16, 9).unwrap())
        );
    }

    #[test]
    fn fits_a_size_larger_than_the_screen_by_its_ratio() {
        assert_eq!(
            fit_content(1920, 1080, ContentArea::size(2560, 1080).unwrap()),
            fit_content(1920, 1080, ContentArea::ratio(2560, 1080).unwrap())
        );
    }

    #[test]
    fn an_exact_size_follows_a_window_of_that_size() {
        let size = ContentArea::size(1920, 1080).unwrap();
        let below_title_bar = ScreenRect {
            left: 320,
            top: 196,
            right: 2240,
            bottom: 1276,
        };
        assert_eq!(
            place_content(2560, 1440, size, Some(below_title_bar)),
            Some(below_title_bar)
        );
        let other_size = ScreenRect {
            bottom: 1275,
            ..below_title_bar
        };
        assert_eq!(
            place_content(2560, 1440, size, Some(other_size)),
            fit_content(2560, 1440, size)
        );
        let off_screen = ScreenRect {
            left: 3000,
            right: 4920,
            ..below_title_bar
        };
        assert_eq!(
            place_content(2560, 1440, size, Some(off_screen)),
            fit_content(2560, 1440, size)
        );
        assert_eq!(
            place_content(2560, 1440, size, None),
            fit_content(2560, 1440, size)
        );
    }

    #[test]
    fn a_ratio_ignores_the_window() {
        let ratio = ContentArea::ratio(16, 9).unwrap();
        let window = ScreenRect {
            left: 0,
            top: 0,
            right: 1600,
            bottom: 900,
        };
        assert_eq!(
            place_content(2560, 1440, ratio, Some(window)),
            fit_content(2560, 1440, ratio)
        );
    }

    #[test]
    fn parses_ratios_and_sizes() {
        assert_eq!(ContentArea::parse(" 21 : 9 "), ContentArea::ratio(21, 9));
        assert_eq!(
            ContentArea::parse("2560x1080"),
            ContentArea::size(2560, 1080)
        );
        assert_eq!(
            ContentArea::parse("1920 X 1080"),
            ContentArea::size(1920, 1080)
        );
        assert_eq!(
            ContentArea::parse("1920×1080"),
            ContentArea::size(1920, 1080)
        );
        assert_eq!(ContentArea::parse("16:9:4"), None);
        assert_eq!(ContentArea::parse("0x1080"), None);
        assert_eq!(ContentArea::parse("5000x1000"), None);
        assert_eq!(ContentArea::parse("1920"), None);
        for area in ["21:9", "2560x1080"] {
            assert_eq!(ContentArea::parse(area).unwrap().to_string(), area);
        }
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
    fn insets_only_the_edges_that_border_a_bar() {
        let pillarboxed = ScreenRect {
            left: 440,
            top: 0,
            right: 3000,
            bottom: 1440,
        };
        assert_eq!(
            inset_bar_edges(pillarboxed, 3440, 1440, 8),
            ScreenRect {
                left: 448,
                top: 0,
                right: 2992,
                bottom: 1440,
            }
        );
        assert_eq!(
            inset_bar_edges(pillarboxed, 3440, 1440, 10_000),
            ScreenRect {
                left: 800,
                top: 0,
                right: 2640,
                bottom: 1440,
            },
            "the inset is capped at a quarter of the shorter side"
        );
        assert_eq!(inset_bar_edges(pillarboxed, 3440, 1440, 0), pillarboxed);
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
