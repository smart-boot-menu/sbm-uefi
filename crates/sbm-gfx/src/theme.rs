use crate::draw::Rgba;

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub dark: bool,
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
            bg: Rgba::rgb(0xF3, 0xF5, 0xF8),
            header: Rgba::rgb(0xFF, 0xFF, 0xFF),
            card: Rgba::rgb(0xFF, 0xFF, 0xFF),
            card_selected: Rgba::rgb(0xE7, 0xF0, 0xFF),
            text: Rgba::rgb(0x1A, 0x1D, 0x21),
            muted: Rgba::rgb(0x5E, 0x6A, 0x75),
            accent: Rgba::rgb(0x0B, 0x57, 0xD0),
            danger: Rgba::rgb(0xC5, 0x22, 0x1F),
            ok: Rgba::rgb(0x0D, 0x7A, 0x4F),
            border: Rgba::rgb(0xD5, 0xDC, 0xE3),
            shadow: Rgba::rgb(0xD0, 0xD5, 0xDC),
        }
    }

    pub fn from_dark(dark: bool) -> Self {
        if dark { Self::dark() } else { Self::light() }
    }
}
