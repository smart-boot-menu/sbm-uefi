use crate::draw::Rgba;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    pub dark: bool,
    pub high_contrast: bool,
    pub bg: Rgba,
    pub header: Rgba,
    pub card: Rgba,
    pub card_selected: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub accent: Rgba,
    pub danger: Rgba,
    pub ok: Rgba,
    pub border: Rgba,
    pub shadow: Rgba,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            dark: true,
            high_contrast: false,
            bg: Rgba::rgb(0x10, 0x14, 0x1A),
            header: Rgba::rgb(0x16, 0x1B, 0x22),
            card: Rgba::rgb(0x1C, 0x23, 0x30),
            card_selected: Rgba::rgb(0x24, 0x30, 0x44),
            text: Rgba::rgb(0xF2, 0xF5, 0xF8),
            muted: Rgba::rgb(0x9A, 0xA6, 0xB2),
            accent: Rgba::rgb(0x4C, 0x8D, 0xFF),
            danger: Rgba::rgb(0xFF, 0x5D, 0x5D),
            ok: Rgba::rgb(0x3D, 0xDC, 0x97),
            border: Rgba::rgb(0x2E, 0x38, 0x48),
            shadow: Rgba::rgb(0x08, 0x0A, 0x0D),
        }
    }

    pub fn light() -> Self {
        Self {
            dark: false,
            high_contrast: false,
            bg: Rgba::rgb(0xF3, 0xF5, 0xF8),
            header: Rgba::rgb(0xFF, 0xFF, 0xFF),
            card: Rgba::rgb(0xFF, 0xFF, 0xFF),
            card_selected: Rgba::rgb(0xE7, 0xF0, 0xFF),
            text: Rgba::rgb(0x1A, 0x1D, 0x21),
            muted: Rgba::rgb(0x52, 0x5E, 0x69),
            accent: Rgba::rgb(0x0B, 0x57, 0xD0),
            danger: Rgba::rgb(0xC5, 0x22, 0x1F),
            ok: Rgba::rgb(0x0D, 0x7A, 0x4F),
            border: Rgba::rgb(0xC9, 0xD2, 0xDC),
            shadow: Rgba::rgb(0xD0, 0xD5, 0xDC),
        }
    }

    /// WCAG AAA high-contrast dark theme (> 15:1 primary text contrast).
    pub fn high_contrast_dark() -> Self {
        Self {
            dark: true,
            high_contrast: true,
            bg: Rgba::rgb(0x00, 0x00, 0x00),
            header: Rgba::rgb(0x0A, 0x0C, 0x10),
            card: Rgba::rgb(0x0D, 0x10, 0x16),
            card_selected: Rgba::rgb(0x14, 0x26, 0x47),
            text: Rgba::rgb(0xFF, 0xFF, 0xFF),
            muted: Rgba::rgb(0xD2, 0xDC, 0xE6),
            accent: Rgba::rgb(0x7A, 0xB8, 0xFF),
            danger: Rgba::rgb(0xFF, 0x80, 0x80),
            ok: Rgba::rgb(0x5E, 0xF2, 0xB0),
            border: Rgba::rgb(0xA8, 0xB8, 0xCC),
            shadow: Rgba::rgb(0x00, 0x00, 0x00),
        }
    }

    /// WCAG AAA high-contrast light theme (> 15:1 primary text contrast).
    pub fn high_contrast_light() -> Self {
        Self {
            dark: false,
            high_contrast: true,
            bg: Rgba::rgb(0xFF, 0xFF, 0xFF),
            header: Rgba::rgb(0xF5, 0xF7, 0xFA),
            card: Rgba::rgb(0xFF, 0xFF, 0xFF),
            card_selected: Rgba::rgb(0xD9, 0xE8, 0xFF),
            text: Rgba::rgb(0x00, 0x00, 0x00),
            muted: Rgba::rgb(0x2B, 0x33, 0x3C),
            accent: Rgba::rgb(0x00, 0x3B, 0x99),
            danger: Rgba::rgb(0x9E, 0x00, 0x00),
            ok: Rgba::rgb(0x00, 0x5C, 0x36),
            border: Rgba::rgb(0x22, 0x28, 0x31),
            shadow: Rgba::rgb(0xD5, 0xDC, 0xE3),
        }
    }

    pub fn from_dark(dark: bool) -> Self {
        if dark { Self::dark() } else { Self::light() }
    }

    pub fn resolve(dark: bool, high_contrast: bool) -> Self {
        match (dark, high_contrast) {
            (true, false) => Self::dark(),
            (false, false) => Self::light(),
            (true, true) => Self::high_contrast_dark(),
            (false, true) => Self::high_contrast_light(),
        }
    }
}
