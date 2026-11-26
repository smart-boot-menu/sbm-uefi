# UEFI design tokens (v1)

ADR-002 says `sbm-uefi` and `sbm-core` share tokens. `sbm-common` does not export a theme schema yet, so this file is the v1 source for the firmware renderer. Values are sRGB.

| Token | Dark | Light |
|---|---|---|
| `bg` | `#10141A` | `#F3F5F8` |
| `header` | `#161B22` | `#FFFFFF` |
| `card` | `#1C2330` | `#FFFFFF` |
| `card-selected` | `#243044` | `#E7F0FF` |
| `text` | `#F2F5F8` | `#1A1D21` |
| `muted` | `#9AA6B2` | `#5E6A75` |
| `accent` | `#4C8DFF` | `#0B57D0` |
| `danger` | `#FF5D5D` | `#C5221F` |
| `ok` | `#3DDC97` | `#0D7A4F` |
| `border` | `#2E3848` | `#D5DCE3` |
| `shadow` | `#080A0D` | `#D0D5DC` |

Spacing and type scale with the framebuffer height. Body text is 16 px below 1000 px tall, 22 px below 1400, 28 px below 2000, and 36 px above that. The bitmap is the public-domain 8x8 font, scaled with nearest-neighbor integer sampling. Layout inset is one font size. Cards use a 12 px corner radius at 16 px type, scaled with the font.

Keyboard use does not depend on these tokens. The same menu state runs on the text console when GOP is absent.
