//! `FontSource` の実装。TeX のフォント名から、SabiFace を通じて計量と字形を引く。
//!
//! 解決の順序（dvipdfmx と同じ）:
//!
//! 1. `名前.vf` があれば仮想フォント。文字ごとの DVI 断片を実行し、部品の字形を合成して一つの輪郭にする
//! 2. `pdftex.map` に項目があれば Type1（`.pfb`）。符号化は `.enc`、無ければフォント組み込みの符号化
//! 3. `kanjix.map` に項目があれば OpenType。upTeX の文字コードは Unicode なので cmap で引く
//! 4. XDV のネイティブフォントは `[path]` または `[path]:index` をそのまま開く
//!
//! 計量は `名前.tfm`（TFM または JFM）。ファイルの探索は [`Locator`] に任せる（既定は `kpsewhich`）。

use sabidvi_format::{FontDef, NativeFontDef};
use sabidvi_page::FontSource;
use sabiface_glyph::opentype::OpenTypeFont;
use sabiface_glyph::type1::Type1Font;
use sabiface_map::FontMap;
use sabiface_metrics::enc::Encoding;
use sabiface_metrics::jfm::Jfm;
use sabiface_metrics::tfm::Tfm;
use sabiface_metrics::vf::Vf;
use sabirender_display::{Path, Segment};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;

/// ファイルの探索
pub trait Locator {
    fn find(&self, name: &str) -> Option<PathBuf>;
}

/// `kpsewhich` で探す（TeX Live）
#[derive(Default)]
pub struct Kpse {
    cache: RefCell<HashMap<String, Option<PathBuf>>>,
}

impl Locator for Kpse {
    fn find(&self, name: &str) -> Option<PathBuf> {
        if let Some(p) = self.cache.borrow().get(name) {
            return p.clone();
        }
        let direct = PathBuf::from(name);
        let found = if direct.is_absolute() && direct.exists() {
            Some(direct)
        } else {
            let out = Command::new("kpsewhich").arg(name).output().ok();
            out.and_then(|o| {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                (!s.is_empty()).then(|| PathBuf::from(s))
            })
        };
        self.cache
            .borrow_mut()
            .insert(name.to_string(), found.clone());
        found
    }
}

enum Metrics {
    Tfm(Tfm),
    Jfm(Jfm),
}

enum GlyphFont {
    Type1 {
        font: Type1Font,
        enc: Option<Encoding>,
    },
    /// OpenType はファイルを保持し、呼び出しごとに解析する（ttf-parser は借用型）
    OpenType {
        data: Vec<u8>,
        index: u32,
    },
    Virtual(Vf),
    Missing,
}

pub struct SabiFonts<L: Locator> {
    locator: L,
    map: FontMap,
    metrics: RefCell<HashMap<String, Option<Rc<Metrics>>>>,
    glyphs: RefCell<HashMap<String, Rc<GlyphFont>>>,
}

impl<L: Locator> SabiFonts<L> {
    pub fn new(locator: L) -> Self {
        let mut map = FontMap::default();
        for (name, kanji) in [("pdftex.map", false), ("kanjix.map", true)] {
            if let Some(p) = locator.find(name) {
                if let Ok(text) = std::fs::read_to_string(&p) {
                    if kanji {
                        map.add_kanji_map(&text);
                    } else {
                        map.add_pdftex_map(&text);
                    }
                }
            }
        }
        SabiFonts {
            locator,
            map,
            metrics: RefCell::new(HashMap::new()),
            glyphs: RefCell::new(HashMap::new()),
        }
    }

    pub fn map(&self) -> &FontMap {
        &self.map
    }

    fn metrics(&self, name: &str) -> Option<Rc<Metrics>> {
        if let Some(m) = self.metrics.borrow().get(name) {
            return m.clone();
        }
        let loaded = self
            .locator
            .find(&format!("{name}.tfm"))
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|data| {
                Tfm::parse(&data)
                    .ok()
                    .map(Metrics::Tfm)
                    .or_else(|| Jfm::parse(&data).ok().map(Metrics::Jfm))
            });
        let rc = loaded.map(Rc::new);
        self.metrics
            .borrow_mut()
            .insert(name.to_string(), rc.clone());
        rc
    }

    fn glyph_font(&self, name: &str) -> Rc<GlyphFont> {
        if let Some(g) = self.glyphs.borrow().get(name) {
            return g.clone();
        }
        let font = self.load_glyph_font(name).unwrap_or(GlyphFont::Missing);
        let rc = Rc::new(font);
        self.glyphs
            .borrow_mut()
            .insert(name.to_string(), rc.clone());
        rc
    }

    fn load_glyph_font(&self, name: &str) -> Option<GlyphFont> {
        if let Some(p) = self.locator.find(&format!("{name}.vf")) {
            if let Ok(vf) = Vf::parse(&std::fs::read(p).ok()?) {
                return Some(GlyphFont::Virtual(vf));
            }
        }
        if let Some(entry) = self.map.get(name) {
            let file = entry.font_file.clone()?;
            let data = std::fs::read(self.locator.find(&file)?).ok()?;
            if file.ends_with(".pfb") || file.ends_with(".pfa") {
                let font = Type1Font::parse(&data).ok()?;
                let enc = entry
                    .encoding_file
                    .as_ref()
                    .and_then(|e| self.locator.find(e))
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .and_then(|t| Encoding::parse(&t).ok());
                return Some(GlyphFont::Type1 { font, enc });
            }
            return Some(GlyphFont::OpenType { data, index: 0 });
        }
        if let Some(entry) = self.map.get_kanji(name) {
            let (index, file) = split_ttc_index(&entry.font_file);
            let data = std::fs::read(self.locator.find(file)?).ok()?;
            return Some(GlyphFont::OpenType { data, index });
        }
        // map に無ければ同名の pfb を探す
        let data = std::fs::read(self.locator.find(&format!("{name}.pfb"))?).ok()?;
        Some(GlyphFont::Type1 {
            font: Type1Font::parse(&data).ok()?,
            enc: None,
        })
    }

    /// 仮想フォントの 1 文字を合成する。単位は 1000 / em
    fn compose_virtual(&self, vf: &Vf, code: u32) -> Option<Path> {
        let ch = vf.char(code)?;
        let mut out = Path::default();
        let mut h: f64 = 0.0; // fix_word 単位（2^-20 em）
        let mut v: f64 = 0.0;
        let mut w = 0.0;
        let mut x = 0.0;
        let mut y = 0.0;
        let mut z = 0.0;
        let mut stack: Vec<(f64, f64, f64, f64, f64, f64)> = Vec::new();
        let mut font = vf.fonts.first().map(|f| f.number);
        let d = &ch.dvi;
        let mut i = 0;
        let em = 1000.0 / 1048576.0; // fix_word → 1000/em
        let read = |i: &mut usize, n: usize, signed: bool| -> i64 {
            let mut v: i64 = 0;
            for k in 0..n {
                v = (v << 8) | d.get(*i + k).copied().unwrap_or(0) as i64;
            }
            *i += n;
            if signed && n < 8 && v & (1 << (8 * n - 1)) != 0 {
                v -= 1 << (8 * n);
            }
            v
        };
        while i < d.len() {
            let op = d[i];
            i += 1;
            match op {
                0..=127 | 128..=131 | 133..=136 => {
                    let c = if op < 128 {
                        op as u32
                    } else {
                        read(
                            &mut i,
                            (op - if op < 133 { 127 } else { 132 }) as usize,
                            false,
                        ) as u32
                    };
                    let advance = if let Some(f) =
                        font.and_then(|n| vf.fonts.iter().find(|f| f.number == n))
                    {
                        let s = f.scale.to_f64();
                        let sub = FontDef {
                            number: f.number,
                            checksum: f.checksum,
                            scaled_size: 0,
                            design_size: 0,
                            area: f.area.clone(),
                            name: f.name.clone(),
                        };
                        if let Some((path, units_to_em)) = self.glyph(&sub, c) {
                            let k = units_to_em * s * 1000.0;
                            let ox = h * em;
                            let oy = -v * em;
                            out.segments.extend(
                                path.segments
                                    .iter()
                                    .map(|seg| scale_offset(*seg, k, ox, oy)),
                            );
                        }
                        self.char_width(&sub, c)
                            .map(|wd| wd * s * 1048576.0)
                            .unwrap_or(0.0)
                    } else {
                        0.0
                    };
                    if op < 133 {
                        h += advance;
                    }
                }
                132 | 137 => {
                    let ht = read(&mut i, 4, true) as f64;
                    let wd = read(&mut i, 4, true) as f64;
                    if ht > 0.0 && wd > 0.0 {
                        out.segments
                            .extend(Path::rect(h * em, -v * em, wd * em, ht * em).segments);
                    }
                    if op == 132 {
                        h += wd;
                    }
                }
                138 => {}
                141 => stack.push((h, v, w, x, y, z)),
                142 => {
                    if let Some(s) = stack.pop() {
                        (h, v, w, x, y, z) = s;
                    }
                }
                143..=146 => h += read(&mut i, (op - 142) as usize, true) as f64,
                147 => h += w,
                148..=151 => {
                    w = read(&mut i, (op - 147) as usize, true) as f64;
                    h += w;
                }
                152 => h += x,
                153..=156 => {
                    x = read(&mut i, (op - 152) as usize, true) as f64;
                    h += x;
                }
                157..=160 => v += read(&mut i, (op - 156) as usize, true) as f64,
                161 => v += y,
                162..=165 => {
                    y = read(&mut i, (op - 161) as usize, true) as f64;
                    v += y;
                }
                166 => v += z,
                167..=170 => {
                    z = read(&mut i, (op - 166) as usize, true) as f64;
                    v += z;
                }
                171..=234 => font = Some((op - 171) as u32),
                235..=238 => font = Some(read(&mut i, (op - 234) as usize, false) as u32),
                239..=242 => {
                    let n = read(&mut i, (op - 238) as usize, false) as usize;
                    i += n;
                }
                _ => break,
            }
        }
        Some(out)
    }
}

fn split_ttc_index(file: &str) -> (u32, &str) {
    // ":0:font.ttc" の形
    if let Some(rest) = file.strip_prefix(':') {
        if let Some((idx, name)) = rest.split_once(':') {
            return (idx.parse().unwrap_or(0), name);
        }
    }
    (0, file)
}

fn scale_offset(seg: Segment, k: f64, ox: f64, oy: f64) -> Segment {
    match seg {
        Segment::MoveTo(x, y) => Segment::MoveTo(x * k + ox, y * k + oy),
        Segment::LineTo(x, y) => Segment::LineTo(x * k + ox, y * k + oy),
        Segment::CurveTo(a, b, c, d, e, f) => Segment::CurveTo(
            a * k + ox,
            b * k + oy,
            c * k + ox,
            d * k + oy,
            e * k + ox,
            f * k + oy,
        ),
        Segment::Close => Segment::Close,
    }
}

fn to_path(outline: &sabiface_glyph::outline::Outline) -> Path {
    use sabiface_glyph::outline::Segment as S;
    Path {
        segments: outline
            .segments
            .iter()
            .map(|s| match *s {
                S::MoveTo(x, y) => Segment::MoveTo(x, y),
                S::LineTo(x, y) => Segment::LineTo(x, y),
                S::CurveTo(a, b, c, d, e, f) => Segment::CurveTo(a, b, c, d, e, f),
                S::Close => Segment::Close,
            })
            .collect(),
    }
}

/// XDV のフォント名 `[path]`、`[path]:index`、または名前
fn parse_native_name(name: &str) -> (String, u32) {
    let (main, index) = match name.rsplit_once(':') {
        Some((m, i)) if m.ends_with(']') => (m, i.parse().unwrap_or(0)),
        _ => (name, 0),
    };
    let main = main
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(main);
    (main.to_string(), index)
}

impl<L: Locator> FontSource for SabiFonts<L> {
    fn char_width(&self, font: &FontDef, code: u32) -> Option<f64> {
        match &*self.metrics(&font.name)? {
            Metrics::Tfm(t) => t.char_info(code as u16).map(|c| c.width.to_f64()),
            Metrics::Jfm(j) => j.type_info(j.char_type(code)).map(|t| t.width.to_f64()),
        }
    }

    fn glyph(&self, font: &FontDef, code: u32) -> Option<(Path, f64)> {
        let gf = self.glyph_font(&font.name);
        match &*gf {
            GlyphFont::Virtual(vf) => self.compose_virtual(vf, code).map(|p| (p, 0.001)),
            GlyphFont::Type1 { font: t1, enc } => {
                let name = match enc {
                    Some(e) => e.glyph_name(code as u8).map(|s| s.to_string()),
                    None => t1.encoding.get(code as usize).cloned().flatten(),
                }?;
                let g = t1.glyph(&name).ok()?;
                Some((to_path(&g.outline), t1.font_matrix[0]))
            }
            GlyphFont::OpenType { data, index } => {
                let ot = OpenTypeFont::parse(data, *index).ok()?;
                let gid = ot.glyph_index(char::from_u32(code)?)?;
                let g = ot.glyph(gid).ok()?;
                Some((to_path(&g.outline), 1.0 / ot.units_per_em() as f64))
            }
            GlyphFont::Missing => None,
        }
    }

    fn native_glyph(&self, font: &NativeFontDef, glyph_id: u16) -> Option<(Path, f64)> {
        let (file, index) = parse_native_name(&font.name);
        let key = format!("native:{}:{index}", file);
        let gf = {
            let cached = self.glyphs.borrow().get(&key).cloned();
            match cached {
                Some(g) => g,
                None => {
                    let loaded = self
                        .locator
                        .find(&file)
                        .and_then(|p| std::fs::read(p).ok())
                        .map(|data| GlyphFont::OpenType { data, index })
                        .unwrap_or(GlyphFont::Missing);
                    let rc = Rc::new(loaded);
                    self.glyphs.borrow_mut().insert(key, rc.clone());
                    rc
                }
            }
        };
        match &*gf {
            GlyphFont::OpenType { data, index } => {
                let ot = OpenTypeFont::parse(data, *index).ok()?;
                let g = ot.glyph(glyph_id).ok()?;
                Some((to_path(&g.outline), 1.0 / ot.units_per_em() as f64))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_names_are_parsed() {
        assert_eq!(
            parse_native_name("[c:/fonts/lm.otf]"),
            ("c:/fonts/lm.otf".into(), 0)
        );
        assert_eq!(parse_native_name("[x.ttc]:2"), ("x.ttc".into(), 2));
        assert_eq!(
            parse_native_name("Latin Modern Roman"),
            ("Latin Modern Roman".into(), 0)
        );
        assert_eq!(split_ttc_index(":1:a.ttc"), (1, "a.ttc"));
    }
}
