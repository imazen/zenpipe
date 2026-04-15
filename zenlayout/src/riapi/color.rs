//! Color parsing: hex (#RGB, #RRGGBB, #RRGGBBAA) and CSS3 named colors.

use crate::CanvasColor;

/// Parse a color string (hex or CSS3 named) into a `CanvasColor`.
///
/// Accepts:
/// - `#RGB` / `RGB` — 3-digit hex, alpha = 0xFF
/// - `#RGBA` / `RGBA` — 4-digit hex
/// - `#RRGGBB` / `RRGGBB` — 6-digit hex, alpha = 0xFF
/// - `#RRGGBBAA` / `RRGGBBAA` — 8-digit hex
/// - CSS3 named colors (case-insensitive): `red`, `transparent`, etc.
pub fn parse_color(s: &str) -> Option<CanvasColor> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Strip optional leading '#'
    let hex = s.strip_prefix('#').unwrap_or(s);

    // Try hex first
    if let Some(c) = parse_hex(hex) {
        return Some(c);
    }

    // Fall back to named color lookup (case-insensitive)
    lookup_named(s)
}

fn parse_hex(hex: &str) -> Option<CanvasColor> {
    // All chars must be hex digits
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }

    match hex.len() {
        3 => {
            // RGB → RRGGBB, alpha = FF
            let r = expand_nibble(hex.as_bytes()[0])?;
            let g = expand_nibble(hex.as_bytes()[1])?;
            let b = expand_nibble(hex.as_bytes()[2])?;
            Some(CanvasColor::Srgb { r, g, b, a: 255 })
        }
        4 => {
            // RGBA → RRGGBBAA
            let r = expand_nibble(hex.as_bytes()[0])?;
            let g = expand_nibble(hex.as_bytes()[1])?;
            let b = expand_nibble(hex.as_bytes()[2])?;
            let a = expand_nibble(hex.as_bytes()[3])?;
            Some(CanvasColor::Srgb { r, g, b, a })
        }
        6 => {
            let r = parse_byte(&hex[0..2])?;
            let g = parse_byte(&hex[2..4])?;
            let b = parse_byte(&hex[4..6])?;
            Some(CanvasColor::Srgb { r, g, b, a: 255 })
        }
        8 => {
            let r = parse_byte(&hex[0..2])?;
            let g = parse_byte(&hex[2..4])?;
            let b = parse_byte(&hex[4..6])?;
            let a = parse_byte(&hex[6..8])?;
            Some(CanvasColor::Srgb { r, g, b, a })
        }
        _ => None,
    }
}

/// Expand a single hex nibble: 'f' → 0xFF, 'a' → 0xAA.
fn expand_nibble(ch: u8) -> Option<u8> {
    let n = hex_val(ch)?;
    Some(n << 4 | n)
}

fn hex_val(ch: u8) -> Option<u8> {
    match ch {
        b'0'..=b'9' => Some(ch - b'0'),
        b'a'..=b'f' => Some(ch - b'a' + 10),
        b'A'..=b'F' => Some(ch - b'A' + 10),
        _ => None,
    }
}

fn parse_byte(s: &str) -> Option<u8> {
    let hi = hex_val(s.as_bytes()[0])?;
    let lo = hex_val(s.as_bytes()[1])?;
    Some(hi << 4 | lo)
}

fn lookup_named(name: &str) -> Option<CanvasColor> {
    // Binary search on the sorted table.
    // We need a lowercase scratch buffer. CSS3 names are all ASCII and max 20 chars.
    let mut buf = [0u8; 24];
    let name_bytes = name.as_bytes();
    if name_bytes.len() > buf.len() {
        return None;
    }
    for (i, &b) in name_bytes.iter().enumerate() {
        buf[i] = b.to_ascii_lowercase();
    }
    let lower = core::str::from_utf8(&buf[..name_bytes.len()]).ok()?;

    CSS3_COLORS
        .binary_search_by_key(&lower, |&(n, _)| n)
        .ok()
        .map(|idx| {
            let [r, g, b, a] = CSS3_COLORS[idx].1;
            CanvasColor::Srgb { r, g, b, a }
        })
}

/// CSS3 named colors, sorted alphabetically for binary search.
/// Format: (name, [r, g, b, a])
const CSS3_COLORS: &[(&str, [u8; 4])] = &[
    ("aliceblue", [240, 248, 255, 255]),
    ("antiquewhite", [250, 235, 215, 255]),
    ("aqua", [0, 255, 255, 255]),
    ("aquamarine", [127, 255, 212, 255]),
    ("azure", [240, 255, 255, 255]),
    ("beige", [245, 245, 220, 255]),
    ("bisque", [255, 228, 196, 255]),
    ("black", [0, 0, 0, 255]),
    ("blanchedalmond", [255, 235, 205, 255]),
    ("blue", [0, 0, 255, 255]),
    ("blueviolet", [138, 43, 226, 255]),
    ("brown", [165, 42, 42, 255]),
    ("burlywood", [222, 184, 135, 255]),
    ("cadetblue", [95, 158, 160, 255]),
    ("chartreuse", [127, 255, 0, 255]),
    ("chocolate", [210, 105, 30, 255]),
    ("coral", [255, 127, 80, 255]),
    ("cornflowerblue", [100, 149, 237, 255]),
    ("cornsilk", [255, 248, 220, 255]),
    ("crimson", [220, 20, 60, 255]),
    ("cyan", [0, 255, 255, 255]),
    ("darkblue", [0, 0, 139, 255]),
    ("darkcyan", [0, 139, 139, 255]),
    ("darkgoldenrod", [184, 134, 11, 255]),
    ("darkgray", [169, 169, 169, 255]),
    ("darkgreen", [0, 100, 0, 255]),
    ("darkgrey", [169, 169, 169, 255]),
    ("darkkhaki", [189, 183, 107, 255]),
    ("darkmagenta", [139, 0, 139, 255]),
    ("darkolivegreen", [85, 107, 47, 255]),
    ("darkorange", [255, 140, 0, 255]),
    ("darkorchid", [153, 50, 204, 255]),
    ("darkred", [139, 0, 0, 255]),
    ("darksalmon", [233, 150, 122, 255]),
    ("darkseagreen", [143, 188, 139, 255]),
    ("darkslateblue", [72, 61, 139, 255]),
    ("darkslategray", [47, 79, 79, 255]),
    ("darkslategrey", [47, 79, 79, 255]),
    ("darkturquoise", [0, 206, 209, 255]),
    ("darkviolet", [148, 0, 211, 255]),
    ("deeppink", [255, 20, 147, 255]),
    ("deepskyblue", [0, 191, 255, 255]),
    ("dimgray", [105, 105, 105, 255]),
    ("dimgrey", [105, 105, 105, 255]),
    ("dodgerblue", [30, 144, 255, 255]),
    ("firebrick", [178, 34, 34, 255]),
    ("floralwhite", [255, 250, 240, 255]),
    ("forestgreen", [34, 139, 34, 255]),
    ("fuchsia", [255, 0, 255, 255]),
    ("gainsboro", [220, 220, 220, 255]),
    ("ghostwhite", [248, 248, 255, 255]),
    ("gold", [255, 215, 0, 255]),
    ("goldenrod", [218, 165, 32, 255]),
    ("gray", [128, 128, 128, 255]),
    ("green", [0, 128, 0, 255]),
    ("greenyellow", [173, 255, 47, 255]),
    ("grey", [128, 128, 128, 255]),
    ("honeydew", [240, 255, 240, 255]),
    ("hotpink", [255, 105, 180, 255]),
    ("indianred", [205, 92, 92, 255]),
    ("indigo", [75, 0, 130, 255]),
    ("ivory", [255, 255, 240, 255]),
    ("khaki", [240, 230, 140, 255]),
    ("lavender", [230, 230, 250, 255]),
    ("lavenderblush", [255, 240, 245, 255]),
    ("lawngreen", [124, 252, 0, 255]),
    ("lemonchiffon", [255, 250, 205, 255]),
    ("lightblue", [173, 216, 230, 255]),
    ("lightcoral", [240, 128, 128, 255]),
    ("lightcyan", [224, 255, 255, 255]),
    ("lightgoldenrodyellow", [250, 250, 210, 255]),
    ("lightgray", [211, 211, 211, 255]),
    ("lightgreen", [144, 238, 144, 255]),
    ("lightgrey", [211, 211, 211, 255]),
    ("lightpink", [255, 182, 193, 255]),
    ("lightsalmon", [255, 160, 122, 255]),
    ("lightseagreen", [32, 178, 170, 255]),
    ("lightskyblue", [135, 206, 250, 255]),
    ("lightslategray", [119, 136, 153, 255]),
    ("lightslategrey", [119, 136, 153, 255]),
    ("lightsteelblue", [176, 196, 222, 255]),
    ("lightyellow", [255, 255, 224, 255]),
    ("lime", [0, 255, 0, 255]),
    ("limegreen", [50, 205, 50, 255]),
    ("linen", [250, 240, 230, 255]),
    ("magenta", [255, 0, 255, 255]),
    ("maroon", [128, 0, 0, 255]),
    ("mediumaquamarine", [102, 205, 170, 255]),
    ("mediumblue", [0, 0, 205, 255]),
    ("mediumorchid", [186, 85, 211, 255]),
    ("mediumpurple", [147, 112, 219, 255]),
    ("mediumseagreen", [60, 179, 113, 255]),
    ("mediumslateblue", [123, 104, 238, 255]),
    ("mediumspringgreen", [0, 250, 154, 255]),
    ("mediumturquoise", [72, 209, 204, 255]),
    ("mediumvioletred", [199, 21, 133, 255]),
    ("midnightblue", [25, 25, 112, 255]),
    ("mintcream", [245, 255, 250, 255]),
    ("mistyrose", [255, 228, 225, 255]),
    ("moccasin", [255, 228, 181, 255]),
    ("navajowhite", [255, 222, 173, 255]),
    ("navy", [0, 0, 128, 255]),
    ("oldlace", [253, 245, 230, 255]),
    ("olive", [128, 128, 0, 255]),
    ("olivedrab", [107, 142, 35, 255]),
    ("orange", [255, 165, 0, 255]),
    ("orangered", [255, 69, 0, 255]),
    ("orchid", [218, 112, 214, 255]),
    ("palegoldenrod", [238, 232, 170, 255]),
    ("palegreen", [152, 251, 152, 255]),
    ("paleturquoise", [175, 238, 238, 255]),
    ("palevioletred", [219, 112, 147, 255]),
    ("papayawhip", [255, 239, 213, 255]),
    ("peachpuff", [255, 218, 185, 255]),
    ("peru", [205, 133, 63, 255]),
    ("pink", [255, 192, 203, 255]),
    ("plum", [221, 160, 221, 255]),
    ("powderblue", [176, 224, 230, 255]),
    ("purple", [128, 0, 128, 255]),
    ("rebeccapurple", [102, 51, 153, 255]),
    ("red", [255, 0, 0, 255]),
    ("rosybrown", [188, 143, 143, 255]),
    ("royalblue", [65, 105, 225, 255]),
    ("saddlebrown", [139, 69, 19, 255]),
    ("salmon", [250, 128, 114, 255]),
    ("sandybrown", [244, 164, 96, 255]),
    ("seagreen", [46, 139, 87, 255]),
    ("seashell", [255, 245, 238, 255]),
    ("sienna", [160, 82, 45, 255]),
    ("silver", [192, 192, 192, 255]),
    ("skyblue", [135, 206, 235, 255]),
    ("slateblue", [106, 90, 205, 255]),
    ("slategray", [112, 128, 144, 255]),
    ("slategrey", [112, 128, 144, 255]),
    ("snow", [255, 250, 250, 255]),
    ("springgreen", [0, 255, 127, 255]),
    ("steelblue", [70, 130, 180, 255]),
    ("tan", [210, 180, 140, 255]),
    ("teal", [0, 128, 128, 255]),
    ("thistle", [216, 191, 216, 255]),
    ("tomato", [255, 99, 71, 255]),
    ("transparent", [0, 0, 0, 0]),
    ("turquoise", [64, 224, 208, 255]),
    ("violet", [238, 130, 238, 255]),
    ("wheat", [245, 222, 179, 255]),
    ("white", [255, 255, 255, 255]),
    ("whitesmoke", [245, 245, 245, 255]),
    ("yellow", [255, 255, 0, 255]),
    ("yellowgreen", [154, 205, 50, 255]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_table_is_sorted() {
        for w in CSS3_COLORS.windows(2) {
            assert!(
                w[0].0 < w[1].0,
                "CSS3_COLORS not sorted: {:?} >= {:?}",
                w[0].0,
                w[1].0
            );
        }
    }

    #[test]
    fn hex_3_digit() {
        assert_eq!(
            parse_color("f00"),
            Some(CanvasColor::Srgb {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            })
        );
    }

    #[test]
    fn hex_3_digit_with_hash() {
        assert_eq!(
            parse_color("#0af"),
            Some(CanvasColor::Srgb {
                r: 0,
                g: 170,
                b: 255,
                a: 255
            })
        );
    }

    #[test]
    fn hex_4_digit_with_alpha() {
        assert_eq!(
            parse_color("f008"),
            Some(CanvasColor::Srgb {
                r: 255,
                g: 0,
                b: 0,
                a: 136
            })
        );
    }

    #[test]
    fn hex_6_digit() {
        assert_eq!(
            parse_color("ff8000"),
            Some(CanvasColor::Srgb {
                r: 255,
                g: 128,
                b: 0,
                a: 255
            })
        );
    }

    #[test]
    fn hex_6_digit_with_hash() {
        assert_eq!(
            parse_color("#FF8000"),
            Some(CanvasColor::Srgb {
                r: 255,
                g: 128,
                b: 0,
                a: 255
            })
        );
    }

    #[test]
    fn hex_8_digit() {
        assert_eq!(
            parse_color("ff000080"),
            Some(CanvasColor::Srgb {
                r: 255,
                g: 0,
                b: 0,
                a: 128
            })
        );
    }

    #[test]
    fn named_color() {
        assert_eq!(
            parse_color("red"),
            Some(CanvasColor::Srgb {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            })
        );
    }

    #[test]
    fn named_color_case_insensitive() {
        assert_eq!(
            parse_color("DarkSlateGray"),
            Some(CanvasColor::Srgb {
                r: 47,
                g: 79,
                b: 79,
                a: 255
            })
        );
    }

    #[test]
    fn transparent() {
        assert_eq!(
            parse_color("transparent"),
            Some(CanvasColor::Srgb {
                r: 0,
                g: 0,
                b: 0,
                a: 0
            })
        );
    }

    #[test]
    fn empty_returns_none() {
        assert_eq!(parse_color(""), None);
    }

    #[test]
    fn invalid_returns_none() {
        assert_eq!(parse_color("notacolor"), None);
        assert_eq!(parse_color("zzz"), None);
        assert_eq!(parse_color("#12345"), None); // 5 digits invalid
    }
}
