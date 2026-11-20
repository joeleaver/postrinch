//! CSS colours in mail, as far as deciding whether a message has a design
//! of its own (crate::plain) and redrawing one that has not in dark colours
//! (crate::dark) needs: reading a colour value, and finding and replacing
//! the colours inside a declaration's value.

/// A colour: red, green, blue, and alpha from 0 (clear) to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Lightness, 0 (black) to 1 (white), as HSL has it.
    pub fn lightness(self) -> f32 {
        let (max, min) = self.extremes();
        (max as f32 + min as f32) / 510.0
    }

    /// How far from grey: the gap between its strongest and weakest
    /// channel, 0 to 255.
    pub fn chroma(self) -> u8 {
        let (max, min) = self.extremes();
        max - min
    }

    fn extremes(self) -> (u8, u8) {
        let max = self.r.max(self.g).max(self.b);
        let min = self.r.min(self.g).min(self.b);
        (max, min)
    }

    /// Clear, or so nearly white and grey that a page of it reads as a
    /// plain white page: what a message written as text may set as its
    /// background (#fff, #f6f8fa, #e2e2e2) without having a design.
    pub fn is_light_neutral(self) -> bool {
        self.a < 0.05 || (self.lightness() >= 0.85 && self.chroma() <= 24)
    }

    /// The same hue at another HSL lightness.
    pub fn with_lightness(self, lightness: f32) -> Self {
        let (h, s, _) = self.hsl();
        let l = lightness.clamp(0.0, 1.0);
        let q = if l < 0.5 {
            l * (1.0 + s)
        } else {
            l + s - l * s
        };
        let p = 2.0 * l - q;
        let channel = |t: f32| {
            let t = t.rem_euclid(1.0);
            let v = if t < 1.0 / 6.0 {
                p + (q - p) * 6.0 * t
            } else if t < 0.5 {
                q
            } else if t < 2.0 / 3.0 {
                p + (q - p) * (2.0 / 3.0 - t) * 6.0
            } else {
                p
            };
            (v * 255.0).round() as u8
        };
        Self {
            r: channel(h + 1.0 / 3.0),
            g: channel(h),
            b: channel(h - 1.0 / 3.0),
            a: self.a,
        }
    }

    /// Hue (0 to 1, a turn), saturation and lightness.
    fn hsl(self) -> (f32, f32, f32) {
        let [r, g, b] = [self.r, self.g, self.b].map(|c| c as f32 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        let d = max - min;
        if d == 0.0 {
            return (0.0, 0.0, l);
        }
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h / 6.0, s, l)
    }

    /// As CSS: `#rrggbb`, or `rgba(…)` when not opaque.
    pub fn css(self) -> String {
        if self.a >= 1.0 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("rgba({}, {}, {}, {:.2})", self.r, self.g, self.b, self.a)
        }
    }
}

/// A colour value (lower-cased, trimmed): `#rgb`, `#rgba`, `#rrggbb`,
/// `#rrggbbaa`, `rgb()`, `rgba()`, or a name mail uses. `None` for anything
/// else (`inherit`, `currentcolor`, a name it does not know).
pub fn parse(value: &str) -> Option<Rgba> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(args) = value
        .strip_prefix("rgba(")
        .or_else(|| value.strip_prefix("rgb("))
    {
        return parse_rgb(args.strip_suffix(')')?);
    }
    named(value)
}

/// A colour written without its `#`, as old attributes allow
/// (`bgcolor="ffffff"`); otherwise as [`parse`].
pub fn parse_attribute(value: &str) -> Option<Rgba> {
    parse(value).or_else(|| parse_hex(value.trim()))
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize, len: usize| u8::from_str_radix(&hex[i * len..(i + 1) * len], 16).ok();
    let (len, count) = match hex.len() {
        3 => (1, 3),
        4 => (1, 4),
        6 => (2, 3),
        8 => (2, 4),
        _ => return None,
    };
    let mut channels = [255u8; 4];
    for (i, channel) in channels.iter_mut().enumerate().take(count) {
        let v = digit(i, len)?;
        *channel = if len == 1 { v * 17 } else { v };
    }
    Some(Rgba {
        r: channels[0],
        g: channels[1],
        b: channels[2],
        a: channels[3] as f32 / 255.0,
    })
}

fn parse_rgb(args: &str) -> Option<Rgba> {
    let parts: Vec<&str> = args
        .split([',', ' ', '/'])
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() < 3 {
        return None;
    }
    let channel = |p: &str| -> Option<u8> {
        match p.strip_suffix('%') {
            Some(pct) => Some((pct.parse::<f32>().ok()? * 2.55).round().clamp(0.0, 255.0) as u8),
            None => Some(p.parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8),
        }
    };
    let a = match parts.get(3) {
        Some(p) => match p.strip_suffix('%') {
            Some(pct) => pct.parse::<f32>().ok()? / 100.0,
            None => p.parse::<f32>().ok()?,
        },
        None => 1.0,
    };
    Some(Rgba {
        r: channel(parts[0])?,
        g: channel(parts[1])?,
        b: channel(parts[2])?,
        a: a.clamp(0.0, 1.0),
    })
}

/// The names mail is written with, Outlook's `windowtext` among them.
fn named(name: &str) -> Option<Rgba> {
    let rgb = |r, g, b| Some(Rgba::rgb(r, g, b));
    match name {
        "transparent" => Some(Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 0.0,
        }),
        "black" | "windowtext" => rgb(0, 0, 0),
        "white" | "window" => rgb(255, 255, 255),
        "whitesmoke" => rgb(245, 245, 245),
        "ghostwhite" => rgb(248, 248, 255),
        "snow" => rgb(255, 250, 250),
        "gainsboro" => rgb(220, 220, 220),
        "lightgray" | "lightgrey" => rgb(211, 211, 211),
        "silver" => rgb(192, 192, 192),
        "darkgray" | "darkgrey" => rgb(169, 169, 169),
        "gray" | "grey" => rgb(128, 128, 128),
        "dimgray" | "dimgrey" => rgb(105, 105, 105),
        "red" => rgb(255, 0, 0),
        "darkred" => rgb(139, 0, 0),
        "maroon" => rgb(128, 0, 0),
        "orange" => rgb(255, 165, 0),
        "yellow" => rgb(255, 255, 0),
        "olive" => rgb(128, 128, 0),
        "lime" => rgb(0, 255, 0),
        "green" => rgb(0, 128, 0),
        "darkgreen" => rgb(0, 100, 0),
        "teal" => rgb(0, 128, 128),
        "aqua" | "cyan" => rgb(0, 255, 255),
        "blue" => rgb(0, 0, 255),
        "mediumblue" => rgb(0, 0, 205),
        "darkblue" => rgb(0, 0, 139),
        "navy" => rgb(0, 0, 128),
        "royalblue" => rgb(65, 105, 225),
        "steelblue" => rgb(70, 130, 180),
        "purple" => rgb(128, 0, 128),
        "fuchsia" | "magenta" => rgb(255, 0, 255),
        _ => None,
    }
}

/// The colours in a declaration's value, each as its place in `value` and
/// what it is: `#…`, `rgb(…)`, `rgba(…)` and the names [`parse`] knows.
/// Words it does not know as colours (`solid`, `none`, a font) are passed
/// over.
pub fn colours_in(value: &str) -> Vec<(std::ops::Range<usize>, Rgba)> {
    let bytes = value.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let start = i;
        if c == b'#' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                i += 1;
            }
        } else if c.is_ascii_alphabetic() {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
                i += 1;
            }
            if bytes.get(i) == Some(&b'(') {
                let close = value[i..].find(')').map_or(bytes.len(), |e| i + e + 1);
                // Inside a function other than rgb() (url(), a gradient) is
                // not looked into: a gradient's colours mark it as designed
                // already (crate::plain), and a url is not a colour.
                let name = &value[start..i];
                i = close;
                if name != "rgb" && name != "rgba" {
                    continue;
                }
            }
        } else {
            i += 1;
            continue;
        }
        if let Some(colour) = parse(&value[start..i]) {
            found.push((start..i, colour));
        }
    }
    found
}

/// `value` with each colour in it replaced by what `map` makes of it.
pub fn map_colours(value: &str, map: impl Fn(Rgba) -> Rgba) -> String {
    let mut out = String::with_capacity(value.len());
    let mut at = 0;
    for (range, colour) in colours_in(value) {
        out.push_str(&value[at..range.start]);
        out.push_str(&map(colour).css());
        at = range.end;
    }
    out.push_str(&value[at..]);
    out
}

/// The declarations in a block (a `style` attribute, or the inside of a
/// rule's braces): each as its property, lower-cased, and the place of its
/// value in `block`.
pub fn declarations(block: &str) -> Vec<(String, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let mut at = 0;
    for piece in block.split(';') {
        let start = at;
        at += piece.len() + 1;
        let Some(colon) = piece.find(':') else {
            continue;
        };
        let property = piece[..colon].trim().to_ascii_lowercase();
        out.push((property, start + colon + 1..start + piece.len()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_ways_mail_writes_colours() {
        assert_eq!(parse("#fff"), Some(Rgba::rgb(255, 255, 255)));
        assert_eq!(
            parse("#1F497D".to_lowercase().as_str()),
            Some(Rgba::rgb(31, 73, 125))
        );
        assert_eq!(parse("rgb(34, 34, 34)"), Some(Rgba::rgb(34, 34, 34)));
        assert_eq!(parse("rgba(0,0,0,0.54)").map(|c| c.a), Some(0.54));
        assert_eq!(parse("#0000").map(|c| c.a), Some(0.0));
        assert_eq!(parse("windowtext"), Some(Rgba::rgb(0, 0, 0)));
        assert_eq!(parse_attribute("ffffff"), Some(Rgba::rgb(255, 255, 255)));
        assert_eq!(parse("inherit"), None);
    }

    #[test]
    fn finds_colours_in_shorthands() {
        let value = " 1px solid #dadce0";
        let found = colours_in(value);
        assert_eq!(found.len(), 1);
        assert_eq!(&value[found[0].0.clone()], "#dadce0");
        assert!(colours_in("url(https://x/white.png) no-repeat").is_empty());
        assert_eq!(
            map_colours("rgb(1, 2, 3) none", |_| Rgba::rgb(255, 0, 0)),
            "#ff0000 none"
        );
    }

    #[test]
    fn white_and_pale_greys_are_light_neutral() {
        for c in ["#fff", "#f6f8fa", "#e2e2e2", "transparent", "rgba(0,0,0,0)"] {
            assert!(parse(c).unwrap().is_light_neutral(), "{c}");
        }
        for c in ["#0b57d0", "#fff2cc", "#cccccc", "yellow", "#3c4043"] {
            assert!(!parse(c).unwrap().is_light_neutral(), "{c}");
        }
    }

    #[test]
    fn lightness_keeps_the_hue() {
        let blue = Rgba::rgb(31, 73, 125).with_lightness(0.75);
        assert!((blue.lightness() - 0.75).abs() < 0.01);
        assert!(blue.b > blue.g && blue.g > blue.r);
    }
}
