//! Font optimization: `src/fonts.txt` lists the fonts an app uses, one a
//! line, and the build writes their `@font-face` rules (`font-display:
//! swap`) into the app's CSS, a fallback face with the web font's metrics
//! (`size-adjust` and the overrides, so the swap does not shift the page),
//! a `--font-<name>` custom property naming both, and a `preload` link per
//! file in every page's head.
//!
//! ```text
//! # family  file (in static/fonts) or google  weights  [italic]  [serif|mono]
//! Inter     inter.woff2        100-900
//! Lora      lora-400.ttf       400 italic  serif
//! Open_Sans google             400 700
//! ```
//!
//! `google` is the opt-in: `wisp build` downloads the Latin subset of those
//! weights from Google Fonts once, into `static/fonts/<name>-<weight>.woff2`,
//! and the app serves them itself from then on. A `_` in a family is a space.

/// What a family falls back to while its font loads: the system font whose
/// metrics the fallback face is sized by.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Generic {
    Sans,
    Serif,
    Mono,
}

impl Generic {
    /// The local font, its average character width (OS/2 `xAvgCharWidth`
    /// over 2048 units) and the CSS generic family.
    fn local(self) -> (&'static str, f64, &'static str) {
        match self {
            Generic::Sans => ("Arial", 904.0 / 2048.0, "sans-serif"),
            Generic::Serif => ("Times New Roman", 821.0 / 2048.0, "serif"),
            Generic::Mono => ("Courier New", 1229.0 / 2048.0, "monospace"),
        }
    }
}

/// One line of `src/fonts.txt`.
#[derive(Debug, PartialEq)]
pub struct Line {
    pub family: String,
    /// A file under `static/fonts`, or `None` for `google`.
    pub file: Option<String>,
    /// `400`, or `100 900` for a range.
    pub weights: Vec<String>,
    pub italic: bool,
    pub generic: Generic,
}

/// A font file and what it is for: a line, or one weight of a `google` line.
#[derive(Debug, PartialEq)]
pub struct Face {
    pub family: String,
    /// Under `static/fonts`.
    pub file: String,
    pub weight: String,
    pub italic: bool,
    pub generic: Generic,
}

/// The name of a family in a file or a property: `Open Sans` is `open-sans`.
pub fn slug(family: &str) -> String {
    let s: String = family
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    s.split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// The lines of `src` (blank lines and `#` comments skipped).
pub fn parse(src: &str) -> Result<Vec<Line>, String> {
    let mut out = Vec::new();
    for (n, line) in src.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let at = |why: &str| format!("src/fonts.txt:{}: {why}", n + 1);
        let mut it = line.split_whitespace();
        let family = it.next().unwrap_or("").replace('_', " ");
        let source = it
            .next()
            .ok_or_else(|| at("a line is `family file-or-google [weights]`"))?;
        let mut l = Line {
            family,
            file: (source != "google").then(|| source.to_string()),
            weights: Vec::new(),
            italic: false,
            generic: Generic::Sans,
        };
        for w in it {
            match w {
                "italic" => l.italic = true,
                "sans" => l.generic = Generic::Sans,
                "serif" => l.generic = Generic::Serif,
                "mono" => l.generic = Generic::Mono,
                _ => {
                    let (a, b) = w.split_once('-').unwrap_or((w, ""));
                    let num = |s: &str| {
                        !s.is_empty() && s.len() <= 4 && s.bytes().all(|c| c.is_ascii_digit())
                    };
                    if !num(a) || !(b.is_empty() || num(b)) {
                        return Err(at(&format!(
                            "`{w}` is not a weight (400, 100-900), italic, sans, serif or mono"
                        )));
                    }
                    l.weights.push(if b.is_empty() {
                        a.into()
                    } else {
                        format!("{a} {b}")
                    });
                }
            }
        }
        if l.weights.is_empty() {
            l.weights.push("400".into());
        }
        if l.file.is_some() && l.weights.len() > 1 {
            return Err(at(
                "a file is one weight (or one range, 100-900); list a file per line",
            ));
        }
        if l.file
            .as_ref()
            .is_some_and(|f| f.contains("..") || f.starts_with('/'))
        {
            return Err(at("the file is a name under static/fonts"));
        }
        if l.file.is_none() && l.weights.iter().any(|w| w.contains(' ')) {
            return Err(at("google takes weights one by one (400 700)"));
        }
        out.push(l);
    }
    Ok(out)
}

/// The font files the lines name, a `google` line as a file per weight.
pub fn faces(lines: &[Line]) -> Vec<Face> {
    let mut out = Vec::new();
    for l in lines {
        for w in &l.weights {
            let file = l.file.clone().unwrap_or_else(|| {
                let it = if l.italic { "-italic" } else { "" };
                format!("{}-{w}{it}.woff2", slug(&l.family))
            });
            out.push(Face {
                family: l.family.clone(),
                file,
                weight: w.clone(),
                italic: l.italic,
                generic: l.generic,
            });
        }
    }
    out
}

/// What a font says about its size: from its `head`, `hhea` and `OS/2`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub upm: f64,
    pub ascent: f64,
    pub descent: f64,
    pub line_gap: f64,
    pub x_avg: f64,
}

/// The metrics of a TrueType or OpenType file (not a `.woff`, whose tables
/// are compressed).
pub fn metrics(b: &[u8]) -> Option<Metrics> {
    let u16_at = |i: usize| b.get(i..i + 2).map(|v| u16::from_be_bytes([v[0], v[1]]));
    let u32_at = |i: usize| {
        b.get(i..i + 4)
            .map(|v| u32::from_be_bytes([v[0], v[1], v[2], v[3]]) as usize)
    };
    if !matches!(b.get(..4)?, [0, 1, 0, 0] | b"OTTO" | b"true") {
        return None;
    }
    let table = |tag: &[u8; 4]| {
        (0..u16_at(4)? as usize).find_map(|k| {
            let r = 12 + k * 16;
            (b.get(r..r + 4)? == tag).then(|| u32_at(r + 8))?
        })
    };
    let i16_at = |i: usize| u16_at(i).map(|v| f64::from(v as i16));
    let (head, hhea, os2) = (table(b"head")?, table(b"hhea")?, table(b"OS/2")?);
    let m = Metrics {
        upm: f64::from(u16_at(head + 18)?),
        ascent: i16_at(hhea + 4)?,
        descent: i16_at(hhea + 6)?,
        line_gap: i16_at(hhea + 8)?,
        x_avg: i16_at(os2 + 2)?,
    };
    (m.upm > 0.0 && m.x_avg > 0.0).then_some(m)
}

fn format_of(file: &str) -> (&'static str, &'static str) {
    match file.rsplit('.').next().unwrap_or("") {
        "woff2" => ("woff2", "font/woff2"),
        "woff" => ("woff", "font/woff"),
        "otf" => ("opentype", "font/otf"),
        _ => ("truetype", "font/ttf"),
    }
}

fn pct(v: f64) -> String {
    format!("{:.2}%", v * 100.0)
}

/// The CSS: a face per file, and per family its fallback face (when
/// `metrics` knows the family's font) and `--font-<name>`. `base` is the
/// path the app is served under.
pub fn css(faces: &[Face], base: &str, metrics_of: impl Fn(&Face) -> Option<Metrics>) -> String {
    let mut out = String::new();
    let mut done: Vec<&str> = Vec::new();
    for f in faces {
        let (format, _) = format_of(&f.file);
        let style = if f.italic { "italic" } else { "normal" };
        out.push_str(&format!(
            "@font-face{{font-family:\"{}\";src:url({base}/fonts/{}) format(\"{format}\");font-weight:{};font-style:{style};font-display:swap}}\n",
            f.family, f.file, f.weight
        ));
    }
    for f in faces {
        if done.contains(&f.family.as_str()) {
            continue;
        }
        done.push(&f.family);
        let (local, local_avg, generic) = f.generic.local();
        let mut stack = format!("\"{}\"", f.family);
        if let Some(m) = faces
            .iter()
            .filter(|g| g.family == f.family)
            .find_map(&metrics_of)
        {
            let adjust = (m.x_avg / m.upm) / local_avg;
            out.push_str(&format!(
                "@font-face{{font-family:\"{} Fallback\";src:local(\"{local}\");size-adjust:{};ascent-override:{};descent-override:{};line-gap-override:{}}}\n",
                f.family,
                pct(adjust),
                pct(m.ascent / (m.upm * adjust)),
                pct(m.descent.abs() / (m.upm * adjust)),
                pct(m.line_gap / (m.upm * adjust))
            ));
            stack.push_str(&format!(",\"{} Fallback\"", f.family));
        }
        out.push_str(&format!(
            ":root{{--font-{}:{stack},{generic}}}\n",
            slug(&f.family)
        ));
    }
    out
}

/// A `preload` link for each file that is a `woff2` or `woff`, for a head.
pub fn preloads(faces: &[Face], base: &str) -> String {
    let mut out = String::new();
    for f in faces
        .iter()
        .filter(|f| matches!(format_of(&f.file).0, "woff2" | "woff"))
    {
        out.push_str(&format!(
            "<link rel=\"preload\" href=\"{base}/fonts/{}\" as=\"font\" type=\"{}\" crossorigin>\n",
            f.file,
            format_of(&f.file).1
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_faces() {
        let lines =
            parse("# fonts\nInter inter.woff2 100-900\nOpen_Sans google 400 700 italic serif\n")
                .unwrap();
        assert_eq!(lines[0].weights, ["100 900"]);
        assert_eq!(lines[1].family, "Open Sans");
        let faces = faces(&lines);
        assert_eq!(faces.len(), 3);
        assert_eq!(faces[1].file, "open-sans-400-italic.woff2");
        assert_eq!(faces[2].generic, Generic::Serif);
        assert!(parse("Inter").is_err());
        assert!(parse("Inter a.woff2 bold").is_err());
        assert!(parse("Inter a.woff2 400 700").is_err());
        assert!(parse("Inter ../a.woff2").is_err());
    }

    /// A font with just the three tables `metrics` reads.
    fn sfnt(upm: u16, ascent: i16, descent: i16, gap: i16, x_avg: i16) -> Vec<u8> {
        let mut b = vec![0, 1, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0];
        for (tag, at) in [(b"head", 60), (b"hhea", 100), (b"OS/2", 140)] {
            b.extend_from_slice(tag);
            b.extend_from_slice(&[0; 4]);
            b.extend_from_slice(&(at as u32).to_be_bytes());
            b.extend_from_slice(&[0; 4]);
        }
        b.resize(200, 0);
        b[60 + 18..60 + 20].copy_from_slice(&upm.to_be_bytes());
        for (k, v) in [ascent, descent, gap].into_iter().enumerate() {
            b[100 + 4 + 2 * k..100 + 6 + 2 * k].copy_from_slice(&v.to_be_bytes());
        }
        b[142..144].copy_from_slice(&x_avg.to_be_bytes());
        b
    }

    #[test]
    fn reads_metrics_and_writes_css() {
        let m = metrics(&sfnt(1000, 900, -200, 0, 500)).unwrap();
        assert_eq!(
            (m.upm, m.ascent, m.descent, m.x_avg),
            (1000.0, 900.0, -200.0, 500.0)
        );
        assert!(metrics(b"wOF2....").is_none());
        let faces = faces(&parse("Inter inter.woff2 400\nLora lora.woff2 400 serif\n").unwrap());
        let out = css(&faces, "/app", |f| (f.family == "Inter").then_some(m));
        assert!(out.contains("src:url(/app/fonts/inter.woff2) format(\"woff2\");font-weight:400;font-style:normal;font-display:swap"));
        // 0.5 / (904 / 2048) = 113.27%; the ascent is 0.9 of that.
        assert!(out.contains("font-family:\"Inter Fallback\";src:local(\"Arial\");size-adjust:113.27%;ascent-override:79.45%"), "{out}");
        assert!(out.contains(":root{--font-inter:\"Inter\",\"Inter Fallback\",sans-serif}"));
        assert!(out.contains(":root{--font-lora:\"Lora\",serif}"));
        let links = preloads(&faces, "");
        assert!(links.contains("<link rel=\"preload\" href=\"/fonts/lora.woff2\" as=\"font\" type=\"font/woff2\" crossorigin>"));
    }
}
