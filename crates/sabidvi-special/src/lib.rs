//! `\special` の解釈。dvipdfmx が受け付ける方言（`spc_pdfm.c`、`spc_xtx.c`、`specials.c`）に従う。
//!
//! - `pdf:` … dvipdfmx 固有。描画に関わるものは型付きで返し、注釈・しおり・文書情報などは `Ignored` として名前だけ保持する
//! - `color …`、`background …` … dvips 由来の色 special（color.sty の dvipdfmx ドライバもこれを出す）
//! - `x:` … XeTeX（xdvipdfmx）向けの拡張
//! - `papersize=…`、`pdf:pagesize` … 紙面
//! - `ps:` … dvips の PostScript。対象外（`Unsupported`）
//!
//! 寸法の単位: 単位なしは bp。`pt in cm mm bp pc dd cc sp` を認める。`true` は倍率を打ち消す（ここでは無視）。

use sabirender_display::{Color, Matrix};

/// 寸法と変換（dvipdfmx の `transform_info`）
#[derive(Debug, Clone, PartialEq)]
pub struct DimTrans {
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub depth: Option<f64>,
    /// `bbox llx lly urx ury`
    pub bbox: Option<[f64; 4]>,
    /// `matrix a b c d e f`、または scale / xscale / yscale / rotate から合成した行列
    pub matrix: Matrix,
    pub clip: bool,
    pub page: Option<u32>,
}

impl Default for DimTrans {
    fn default() -> Self {
        DimTrans {
            width: None,
            height: None,
            depth: None,
            bbox: None,
            matrix: Matrix::IDENTITY,
            clip: false,
            page: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PdfSpecial {
    /// `pdf:code …`: 現在の変換のまま PDF 演算子を流す
    Code(String),
    /// `pdf:literal [direct] …`: `direct` でなければ現在位置へ平行移動して流す
    Literal {
        direct: bool,
        code: String,
    },
    /// `pdf:bcontent` / `pdf:econtent`
    BeginContent,
    EndContent,
    /// `pdf:btrans …` / `pdf:etrans`
    BeginTransform(DimTrans),
    EndTransform,
    /// `pdf:bcolor …`: 塗りと線の色を積む。`pdf:ecolor` で戻す。`pdf:scolor` は積まずに置き換える
    BeginColor {
        fill: Option<Color>,
        stroke: Option<Color>,
    },
    EndColor,
    SetColor {
        fill: Option<Color>,
        stroke: Option<Color>,
    },
    /// `pdf:bxobj @name …` / `pdf:exobj` / `pdf:uxobj @name …`
    BeginXObject {
        ident: String,
        dims: DimTrans,
    },
    EndXObject,
    UseXObject {
        ident: String,
        dims: DimTrans,
    },
    /// `pdf:image [@name] … (file)`
    Image {
        ident: Option<String>,
        dims: DimTrans,
        file: String,
    },
    /// `pdf:obj @name <object>` / `pdf:put @name <object>` / `pdf:stream @name (data) <dict>`: 生のまま保持
    Object {
        ident: String,
        body: String,
    },
    Put {
        ident: String,
        body: String,
    },
    Stream {
        ident: String,
        body: String,
    },
    /// `pdf:pagesize …`
    PageSize(DimTrans),
    /// 描画に関わらないもの（注釈、しおり、文書情報、名前、mapline など）
    Ignored(String),
    /// 名前は知っているが対応していないもの（`bxgstate` など）
    Unsupported(String),
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ColorSpecial {
    Push(Color),
    Pop,
    Set(Color),
    Background(Color),
}

#[derive(Debug, Clone, PartialEq)]
pub enum XtxSpecial {
    Scale(f64, f64),
    Rotate(f64),
    GSave,
    GRestore,
    Ignored(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Special {
    Pdf(PdfSpecial),
    Color(ColorSpecial),
    Xtx(XtxSpecial),
    /// `papersize=W,H`（bp）
    PaperSize(f64, f64),
    /// `landscape`
    Landscape,
    /// `dvipdfmx:config …`
    Config(String),
    /// `sabidvi:mark <名前>`: 位置の目印（数式の基線など）。描画には影響しない
    Mark(String),
    /// dvips の `ps:`、`!`、`"`、`header=` など
    Unsupported(String),
    Unknown(String),
}

/// 1 個の special を解釈する
pub fn parse(raw: &[u8]) -> Special {
    let text = String::from_utf8_lossy(raw);
    let t = text.trim_start();
    if let Some(rest) = t.strip_prefix("pdf:") {
        return Special::Pdf(parse_pdf(rest.trim_start()));
    }
    if let Some(rest) = t.strip_prefix("x:") {
        return Special::Xtx(parse_xtx(rest.trim_start()));
    }
    if let Some(rest) = t.strip_prefix("dvipdfmx:") {
        return Special::Config(rest.trim().to_string());
    }
    if let Some(rest) = t.strip_prefix("sabidvi:mark") {
        return Special::Mark(rest.trim().to_string());
    }
    if let Some(rest) = t.strip_prefix("color ") {
        return Special::Color(parse_color_special(rest.trim()));
    }
    if let Some(rest) = t.strip_prefix("background ") {
        return match parse_colorspec(&mut Cursor::new(rest.trim())) {
            Some(c) => Special::Color(ColorSpecial::Background(c)),
            None => Special::Unknown(text.into_owned()),
        };
    }
    if let Some(rest) = t.strip_prefix("papersize=") {
        let mut it = rest.split(',');
        if let (Some(w), Some(h)) = (it.next(), it.next()) {
            if let (Some(w), Some(h)) = (parse_length_str(w.trim()), parse_length_str(h.trim())) {
                return Special::PaperSize(w, h);
            }
        }
        return Special::Unknown(text.into_owned());
    }
    if t.trim() == "landscape" {
        return Special::Landscape;
    }
    if t.starts_with("ps:")
        || t.starts_with('!')
        || t.starts_with('"')
        || t.starts_with("header=")
        || t.starts_with("psfile=")
        || t.starts_with("PSfile=")
    {
        return Special::Unsupported(text.into_owned());
    }
    Special::Unknown(text.into_owned())
}

// ---------- pdf: ----------

fn parse_pdf(s: &str) -> PdfSpecial {
    let (kw, rest) = split_keyword(s);
    let rest = rest.trim_start();
    match kw {
        "code" => PdfSpecial::Code(rest.to_string()),
        "literal" => {
            let mut direct = false;
            let mut r = rest;
            loop {
                if let Some(x) = r.strip_prefix("direct") {
                    direct = true;
                    r = x.trim_start();
                } else if let Some(x) = r.strip_prefix("reverse") {
                    r = x.trim_start();
                } else {
                    break;
                }
            }
            PdfSpecial::Literal {
                direct,
                code: r.to_string(),
            }
        }
        "bcontent" => PdfSpecial::BeginContent,
        "econtent" => PdfSpecial::EndContent,
        "btrans" | "begintransform" | "begintrans" | "bt" => {
            PdfSpecial::BeginTransform(parse_dimtrans(&mut Cursor::new(rest)))
        }
        "etrans" | "endtransform" | "endtrans" | "et" => PdfSpecial::EndTransform,
        "bcolor" | "begincolor" | "bc" => {
            let (fill, stroke) = parse_color_pair(rest);
            PdfSpecial::BeginColor { fill, stroke }
        }
        "scolor" | "setcolor" | "sc" => {
            let (fill, stroke) = parse_color_pair(rest);
            PdfSpecial::SetColor { fill, stroke }
        }
        "ecolor" | "endcolor" | "ec" => PdfSpecial::EndColor,
        "bgray" | "begingray" | "bg" => {
            let g = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
                .map(Color::Gray);
            PdfSpecial::BeginColor { fill: g, stroke: g }
        }
        "egray" | "endgray" | "eg" => PdfSpecial::EndColor,
        "bxobj" | "beginxobj" | "bform" => {
            let mut c = Cursor::new(rest);
            let ident = c.ident().unwrap_or_default();
            PdfSpecial::BeginXObject {
                ident,
                dims: parse_dimtrans(&mut c),
            }
        }
        "exobj" | "endxobj" | "eform" => PdfSpecial::EndXObject,
        "uxobj" | "usexobj" => {
            let mut c = Cursor::new(rest);
            let ident = c.ident().unwrap_or_default();
            PdfSpecial::UseXObject {
                ident,
                dims: parse_dimtrans(&mut c),
            }
        }
        "image" | "img" | "epdf" => {
            let mut c = Cursor::new(rest);
            let ident = c.ident();
            let dims = parse_dimtrans(&mut c);
            let file = c.pdf_string_or_word().unwrap_or_default();
            PdfSpecial::Image { ident, dims, file }
        }
        "obj" | "object" => {
            let mut c = Cursor::new(rest);
            let ident = c.ident().unwrap_or_default();
            PdfSpecial::Object {
                ident,
                body: c.rest().trim().to_string(),
            }
        }
        "put" => {
            let mut c = Cursor::new(rest);
            let ident = c.ident().unwrap_or_default();
            PdfSpecial::Put {
                ident,
                body: c.rest().trim().to_string(),
            }
        }
        "stream" | "fstream" => {
            let mut c = Cursor::new(rest);
            let ident = c.ident().unwrap_or_default();
            PdfSpecial::Stream {
                ident,
                body: c.rest().trim().to_string(),
            }
        }
        "pagesize" => PdfSpecial::PageSize(parse_dimtrans(&mut Cursor::new(rest))),
        "bgcolor" | "bgc" | "bbc" | "bbg" => PdfSpecial::Ignored(kw.to_string()),
        "annotation" | "annotate" | "annot" | "ann" | "outline" | "out" | "article" | "art"
        | "bead" | "thread" | "destination" | "dest" | "docinfo" | "docview" | "content"
        | "close" | "bop" | "eop" | "link" | "nolink" | "bannot" | "beginann" | "bann"
        | "eannot" | "endann" | "eann" | "tounicode" | "names" | "mapline" | "mapfile"
        | "minorversion" | "majorversion" | "encrypt" | "pageresources" | "trailerid"
        | "xannot" | "extendann" | "xann" => PdfSpecial::Ignored(kw.to_string()),
        "bxgstate" | "exgstate" => PdfSpecial::Unsupported(kw.to_string()),
        _ => PdfSpecial::Unknown(s.to_string()),
    }
}

fn split_keyword(s: &str) -> (&str, &str) {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    (&s[..end], &s[end..])
}

/// `bcolor` の引数: `fill <color> stroke <color>` または `<fill> [<stroke>]`
fn parse_color_pair(s: &str) -> (Option<Color>, Option<Color>) {
    let mut c = Cursor::new(s);
    c.skip_ws();
    if c.starts_with("fill") || c.starts_with("stroke") {
        let mut fill = None;
        let mut stroke = None;
        loop {
            c.skip_ws();
            if c.eat("fill") {
                fill = parse_colorspec(&mut c);
            } else if c.eat("stroke") {
                stroke = parse_colorspec(&mut c);
            } else {
                break;
            }
        }
        return (fill, stroke);
    }
    let fill = parse_colorspec(&mut c);
    c.skip_ws();
    let stroke = if c.at_end() {
        fill
    } else {
        parse_colorspec(&mut c)
    };
    (fill, stroke.or(fill))
}

// ---------- 色 ----------

/// dvipdfmx の色の書式（`spc_util_read_pdfcolor` / `spc_util_read_colorspec`）:
/// `[g]`、`[r g b]`、`[c m y k]`、`gray v`、`rgb r g b`、`cmyk c m y k`、`hsb h s b`、`spot name tint`、dvips の色名
pub fn parse_colorspec(c: &mut Cursor) -> Option<Color> {
    c.skip_ws();
    if c.eat("[") {
        let mut v = Vec::new();
        loop {
            c.skip_ws();
            if c.eat("]") {
                break;
            }
            v.push(c.number()?);
        }
        return match v.len() {
            1 => Some(Color::Gray(v[0])),
            3 => Some(Color::Rgb(v[0], v[1], v[2])),
            4 => Some(Color::Cmyk(v[0], v[1], v[2], v[3])),
            _ => None,
        };
    }
    let word = c.word()?;
    match word.as_str() {
        "gray" | "Gray" => Some(Color::Gray(c.number()?)),
        "rgb" | "RGB" => Some(Color::Rgb(c.number()?, c.number()?, c.number()?)),
        "cmyk" | "CMYK" => Some(Color::Cmyk(
            c.number()?,
            c.number()?,
            c.number()?,
            c.number()?,
        )),
        "hsb" | "HSB" => {
            let (h, s, b) = (c.number()?, c.number()?, c.number()?);
            let (r, g, bl) = hsb_to_rgb(h, s, b);
            Some(Color::Rgb(r, g, bl))
        }
        "spot" => {
            // 名前と濃さ。名前付き色空間は持たないので濃さを灰色にする
            let _name = c.word()?;
            let tint = c.number()?;
            Some(Color::Gray(1.0 - tint))
        }
        _ => named_color(&word),
    }
}

fn hsb_to_rgb(h: f64, s: f64, b: f64) -> (f64, f64, f64) {
    // dvipdfmx spc_util.c の HSB → RGB
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = b * (1.0 - s);
    let q = b * (1.0 - s * f);
    let t = b * (1.0 - s * (1.0 - f));
    match (i as i64).rem_euclid(6) {
        0 => (b, t, p),
        1 => (q, b, p),
        2 => (p, b, t),
        3 => (p, q, b),
        4 => (t, p, b),
        _ => (b, p, q),
    }
}

/// dvips の `dvipsnam.def` にある 68 色（CMYK）
pub fn named_color(name: &str) -> Option<Color> {
    const TABLE: [(&str, [f64; 4]); 68] = [
        ("GreenYellow", [0.15, 0.0, 0.69, 0.0]),
        ("Yellow", [0.0, 0.0, 1.0, 0.0]),
        ("Goldenrod", [0.0, 0.10, 0.84, 0.0]),
        ("Dandelion", [0.0, 0.29, 0.84, 0.0]),
        ("Apricot", [0.0, 0.32, 0.52, 0.0]),
        ("Peach", [0.0, 0.50, 0.70, 0.0]),
        ("Melon", [0.0, 0.46, 0.50, 0.0]),
        ("YellowOrange", [0.0, 0.42, 1.0, 0.0]),
        ("Orange", [0.0, 0.61, 0.87, 0.0]),
        ("BurntOrange", [0.0, 0.51, 1.0, 0.0]),
        ("Bittersweet", [0.0, 0.75, 1.0, 0.24]),
        ("RedOrange", [0.0, 0.77, 0.87, 0.0]),
        ("Mahogany", [0.0, 0.85, 0.87, 0.35]),
        ("Maroon", [0.0, 0.87, 0.68, 0.32]),
        ("BrickRed", [0.0, 0.89, 0.94, 0.28]),
        ("Red", [0.0, 1.0, 1.0, 0.0]),
        ("OrangeRed", [0.0, 1.0, 0.50, 0.0]),
        ("RubineRed", [0.0, 1.0, 0.13, 0.0]),
        ("WildStrawberry", [0.0, 0.96, 0.39, 0.0]),
        ("Salmon", [0.0, 0.53, 0.38, 0.0]),
        ("CarnationPink", [0.0, 0.63, 0.0, 0.0]),
        ("Magenta", [0.0, 1.0, 0.0, 0.0]),
        ("VioletRed", [0.0, 0.81, 0.0, 0.0]),
        ("Rhodamine", [0.0, 0.82, 0.0, 0.0]),
        ("Mulberry", [0.34, 0.90, 0.0, 0.02]),
        ("RedViolet", [0.07, 0.90, 0.0, 0.34]),
        ("Fuchsia", [0.47, 0.91, 0.0, 0.08]),
        ("Lavender", [0.0, 0.48, 0.0, 0.0]),
        ("Thistle", [0.12, 0.59, 0.0, 0.0]),
        ("Orchid", [0.32, 0.64, 0.0, 0.0]),
        ("DarkOrchid", [0.40, 0.80, 0.20, 0.0]),
        ("Purple", [0.45, 0.86, 0.0, 0.0]),
        ("Plum", [0.50, 1.0, 0.0, 0.0]),
        ("Violet", [0.79, 0.88, 0.0, 0.0]),
        ("RoyalPurple", [0.75, 0.90, 0.0, 0.0]),
        ("BlueViolet", [0.86, 0.91, 0.0, 0.04]),
        ("Periwinkle", [0.57, 0.55, 0.0, 0.0]),
        ("CadetBlue", [0.62, 0.57, 0.23, 0.0]),
        ("CornflowerBlue", [0.65, 0.13, 0.0, 0.0]),
        ("MidnightBlue", [0.98, 0.13, 0.0, 0.43]),
        ("NavyBlue", [0.94, 0.54, 0.0, 0.0]),
        ("RoyalBlue", [1.0, 0.50, 0.0, 0.0]),
        ("Blue", [1.0, 1.0, 0.0, 0.0]),
        ("Cerulean", [0.94, 0.11, 0.0, 0.0]),
        ("Cyan", [1.0, 0.0, 0.0, 0.0]),
        ("ProcessBlue", [0.96, 0.0, 0.0, 0.0]),
        ("SkyBlue", [0.62, 0.0, 0.12, 0.0]),
        ("Turquoise", [0.85, 0.0, 0.20, 0.0]),
        ("TealBlue", [0.86, 0.0, 0.34, 0.02]),
        ("Aquamarine", [0.82, 0.0, 0.30, 0.0]),
        ("BlueGreen", [0.85, 0.0, 0.33, 0.0]),
        ("Emerald", [1.0, 0.0, 0.50, 0.0]),
        ("JungleGreen", [0.99, 0.0, 0.52, 0.0]),
        ("SeaGreen", [0.69, 0.0, 0.50, 0.0]),
        ("Green", [1.0, 0.0, 1.0, 0.0]),
        ("ForestGreen", [0.91, 0.0, 0.88, 0.12]),
        ("PineGreen", [0.92, 0.0, 0.59, 0.25]),
        ("LimeGreen", [0.50, 0.0, 1.0, 0.0]),
        ("YellowGreen", [0.44, 0.0, 0.74, 0.0]),
        ("SpringGreen", [0.26, 0.0, 0.76, 0.0]),
        ("OliveGreen", [0.64, 0.0, 0.95, 0.40]),
        ("RawSienna", [0.0, 0.72, 1.0, 0.45]),
        ("Sepia", [0.0, 0.83, 1.0, 0.70]),
        ("Brown", [0.0, 0.81, 1.0, 0.60]),
        ("Tan", [0.14, 0.42, 0.56, 0.0]),
        ("Gray", [0.0, 0.0, 0.0, 0.50]),
        ("Black", [0.0, 0.0, 0.0, 1.0]),
        ("White", [0.0, 0.0, 0.0, 0.0]),
    ];
    TABLE
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| Color::Cmyk(c[0], c[1], c[2], c[3]))
}

fn parse_color_special(s: &str) -> ColorSpecial {
    let mut c = Cursor::new(s);
    c.skip_ws();
    if c.eat("push") {
        return match parse_colorspec(&mut c) {
            Some(col) => ColorSpecial::Push(col),
            None => ColorSpecial::Push(Color::BLACK),
        };
    }
    if c.eat("pop") {
        return ColorSpecial::Pop;
    }
    match parse_colorspec(&mut c) {
        Some(col) => ColorSpecial::Set(col),
        None => ColorSpecial::Set(Color::BLACK),
    }
}

// ---------- 寸法と変換 ----------

/// `width w height h depth d scale s xscale sx yscale sy rotate deg bbox llx lly urx ury matrix a b c d e f clip page n hide`
pub fn parse_dimtrans(c: &mut Cursor) -> DimTrans {
    let mut t = DimTrans::default();
    let mut scale = (1.0, 1.0);
    let mut rotate = 0.0;
    let mut user_matrix: Option<Matrix> = None;
    loop {
        c.skip_ws();
        let Some(key) = c.peek_word() else { break };
        match key.as_str() {
            "width" => {
                c.word();
                t.width = c.length();
            }
            "height" => {
                c.word();
                t.height = c.length();
            }
            "depth" => {
                c.word();
                t.depth = c.length();
            }
            "scale" => {
                c.word();
                if let Some(s) = c.number() {
                    scale = (s, s);
                }
            }
            "xscale" => {
                c.word();
                if let Some(s) = c.number() {
                    scale.0 = s;
                }
            }
            "yscale" => {
                c.word();
                if let Some(s) = c.number() {
                    scale.1 = s;
                }
            }
            "rotate" => {
                c.word();
                if let Some(r) = c.number() {
                    rotate = r;
                }
            }
            "bbox" => {
                c.word();
                let v: Vec<Option<f64>> = (0..4).map(|_| c.length()).collect();
                if v.iter().all(|x| x.is_some()) {
                    t.bbox = Some([v[0].unwrap(), v[1].unwrap(), v[2].unwrap(), v[3].unwrap()]);
                }
            }
            "matrix" => {
                c.word();
                let v: Vec<Option<f64>> = (0..6).map(|_| c.number()).collect();
                if v.iter().all(|x| x.is_some()) {
                    user_matrix = Some(Matrix::new(
                        v[0].unwrap(),
                        v[1].unwrap(),
                        v[2].unwrap(),
                        v[3].unwrap(),
                        v[4].unwrap(),
                        v[5].unwrap(),
                    ));
                }
            }
            "clip" => {
                c.word();
                t.clip = true;
            }
            "page" => {
                c.word();
                t.page = c.number().map(|v| v as u32);
            }
            "hide" | "true" => {
                c.word();
            }
            _ => break,
        }
    }
    t.matrix = match user_matrix {
        Some(m) => m,
        None if rotate == 0.0 && scale == (1.0, 1.0) => Matrix::IDENTITY,
        None => {
            // spc_util.c の make_transmatrix: 回転してから拡大
            let (s, co) = (rotate.to_radians().sin(), rotate.to_radians().cos());
            Matrix::new(
                scale.0 * co,
                scale.0 * s,
                -scale.1 * s,
                scale.1 * co,
                0.0,
                0.0,
            )
        }
    };
    t
}

/// 単位付きの長さ（bp）
pub fn parse_length_str(s: &str) -> Option<f64> {
    Cursor::new(s).length()
}

fn unit_to_bp(unit: &str) -> Option<f64> {
    Some(match unit {
        "pt" => 72.0 / 72.27,
        "in" => 72.0,
        "cm" => 72.0 / 2.54,
        "mm" => 72.0 / 25.4,
        "bp" => 1.0,
        "pc" => 12.0 * 72.0 / 72.27,
        "dd" => 1238.0 / 1157.0 * 72.0 / 72.27,
        "cc" => 12.0 * 1238.0 / 1157.0 * 72.0 / 72.27,
        "sp" => 72.0 / (72.27 * 65536.0),
        _ => return None,
    })
}

// ---------- x: ----------

fn parse_xtx(s: &str) -> XtxSpecial {
    let (kw, rest) = split_keyword(s);
    let mut c = Cursor::new(rest);
    match kw {
        "scale" => {
            let x = c.number().unwrap_or(1.0);
            let y = c.number().unwrap_or(x);
            XtxSpecial::Scale(x, y)
        }
        "rotate" => XtxSpecial::Rotate(c.number().unwrap_or(0.0)),
        "gsave" => XtxSpecial::GSave,
        "grestore" => XtxSpecial::GRestore,
        _ => XtxSpecial::Ignored(kw.to_string()),
    }
}

// ---------- 字句 ----------

pub struct Cursor<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(s: &'a str) -> Self {
        Cursor { s, pos: 0 }
    }

    pub fn rest(&self) -> &'a str {
        &self.s[self.pos..]
    }

    pub fn at_end(&mut self) -> bool {
        self.skip_ws();
        self.pos >= self.s.len()
    }

    pub fn skip_ws(&mut self) {
        while self.pos < self.s.len() && self.s.as_bytes()[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn starts_with(&self, p: &str) -> bool {
        self.rest().starts_with(p)
    }

    fn eat(&mut self, p: &str) -> bool {
        if self.starts_with(p) {
            self.pos += p.len();
            true
        } else {
            false
        }
    }

    fn peek_word(&mut self) -> Option<String> {
        let save = self.pos;
        let w = self.word();
        self.pos = save;
        w
    }

    /// 英数字と `_` `.` `-` `+` の語
    pub fn word(&mut self) -> Option<String> {
        self.skip_ws();
        let start = self.pos;
        let b = self.s.as_bytes();
        while self.pos < b.len()
            && (b[self.pos].is_ascii_alphanumeric()
                || matches!(b[self.pos], b'_' | b'.' | b'-' | b'+'))
        {
            self.pos += 1;
        }
        if self.pos == start {
            return None;
        }
        Some(self.s[start..self.pos].to_string())
    }

    /// `@name`
    pub fn ident(&mut self) -> Option<String> {
        self.skip_ws();
        if !self.starts_with("@") {
            return None;
        }
        self.pos += 1;
        self.word().map(|w| format!("@{w}"))
    }

    pub fn number(&mut self) -> Option<f64> {
        self.skip_ws();
        let b = self.s.as_bytes();
        let start = self.pos;
        while self.pos < b.len() && matches!(b[self.pos], b'0'..=b'9' | b'.' | b'-' | b'+') {
            self.pos += 1;
        }
        let v = self.s[start..self.pos].parse().ok();
        if v.is_none() {
            self.pos = start;
        }
        v
    }

    /// 数と任意の単位（`true` 接頭辞は無視）。単位なしは bp
    pub fn length(&mut self) -> Option<f64> {
        let v = self.number()?;
        let save = self.pos;
        self.skip_ws();
        let _ = self.eat("true");
        self.skip_ws();
        let b = self.s.as_bytes();
        let start = self.pos;
        while self.pos < b.len() && b[self.pos].is_ascii_alphabetic() {
            self.pos += 1;
        }
        let unit = &self.s[start..self.pos];
        match unit_to_bp(unit) {
            Some(u) => Some(v * u),
            None => {
                self.pos = save;
                Some(v)
            }
        }
    }

    /// `(file name)` か、空白までの語
    pub fn pdf_string_or_word(&mut self) -> Option<String> {
        self.skip_ws();
        if self.eat("(") {
            let start = self.pos;
            let mut depth = 1;
            let b = self.s.as_bytes();
            while self.pos < b.len() {
                match b[self.pos] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            let s = self.s[start..self.pos].to_string();
                            self.pos += 1;
                            return Some(s);
                        }
                    }
                    _ => {}
                }
                self.pos += 1;
            }
            return Some(self.s[start..].to_string());
        }
        let start = self.pos;
        let b = self.s.as_bytes();
        while self.pos < b.len() && !b[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
        (self.pos > start).then(|| self.s[start..self.pos].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdf(s: &str) -> PdfSpecial {
        match parse(s.as_bytes()) {
            Special::Pdf(p) => p,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_pgf_style_specials() {
        assert_eq!(
            pdf("pdf:code q 1 0 0 1 0 0 cm"),
            PdfSpecial::Code("q 1 0 0 1 0 0 cm".into())
        );
        assert_eq!(pdf("pdf: bcontent"), PdfSpecial::BeginContent);
        assert_eq!(pdf("pdf:econtent"), PdfSpecial::EndContent);
        assert_eq!(
            pdf("pdf:literal direct 0 0 m 1 1 l S"),
            PdfSpecial::Literal {
                direct: true,
                code: "0 0 m 1 1 l S".into()
            }
        );
        match pdf("pdf:btrans matrix 0.5 0 0 0.5 10 20") {
            PdfSpecial::BeginTransform(t) => {
                assert_eq!(t.matrix, Matrix::new(0.5, 0.0, 0.0, 0.5, 10.0, 20.0))
            }
            other => panic!("{other:?}"),
        }
        match pdf("pdf:btrans rotate 90 scale 2") {
            PdfSpecial::BeginTransform(t) => {
                assert!(
                    (t.matrix.a).abs() < 1e-12
                        && (t.matrix.b - 2.0).abs() < 1e-12
                        && (t.matrix.c + 2.0).abs() < 1e-12
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            pdf("pdf:bcolor [1]"),
            PdfSpecial::BeginColor {
                fill: Some(Color::Gray(1.0)),
                stroke: Some(Color::Gray(1.0))
            }
        );
        assert_eq!(
            pdf("pdf:bcolor fill [1 0 0] stroke [0 0 1]"),
            PdfSpecial::BeginColor {
                fill: Some(Color::Rgb(1.0, 0.0, 0.0)),
                stroke: Some(Color::Rgb(0.0, 0.0, 1.0))
            }
        );
        assert_eq!(pdf("pdf:ecolor"), PdfSpecial::EndColor);
        match pdf("pdf:bxobj @pgfshade1 width 100pt height 20bp") {
            PdfSpecial::BeginXObject { ident, dims } => {
                assert_eq!(ident, "@pgfshade1");
                assert!((dims.width.unwrap() - 100.0 * 72.0 / 72.27).abs() < 1e-9);
                assert_eq!(dims.height, Some(20.0));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            pdf("pdf:uxobj @pgfshade1"),
            PdfSpecial::UseXObject {
                ident: "@pgfshade1".into(),
                dims: DimTrans {
                    matrix: Matrix::IDENTITY,
                    ..Default::default()
                }
            }
        );
        assert_eq!(
            pdf("pdf:put @pgfextgs << /pgfgs1 << /ca 0.5 >> >>"),
            PdfSpecial::Put {
                ident: "@pgfextgs".into(),
                body: "<< /pgfgs1 << /ca 0.5 >> >>".into()
            }
        );
        match pdf("pdf:image width 10cm (fig.pdf)") {
            PdfSpecial::Image { ident, dims, file } => {
                assert_eq!(ident, None);
                assert!((dims.width.unwrap() - 10.0 * 72.0 / 2.54).abs() < 1e-9);
                assert_eq!(file, "fig.pdf");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            pdf("pdf:dest (page.1) [@thispage /XYZ null null null]"),
            PdfSpecial::Ignored("dest".into())
        );
        match pdf("pdf:pagesize width 210mm height 297mm") {
            PdfSpecial::PageSize(t) => {
                assert!((t.width.unwrap() - 210.0 * 72.0 / 25.4).abs() < 1e-9);
                assert!((t.height.unwrap() - 297.0 * 72.0 / 25.4).abs() < 1e-9);
                assert_eq!(t.matrix, Matrix::IDENTITY);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_dvips_color_specials_and_named_colors() {
        assert_eq!(
            parse(b"color push rgb 1 0 0"),
            Special::Color(ColorSpecial::Push(Color::Rgb(1.0, 0.0, 0.0)))
        );
        assert_eq!(parse(b"color pop"), Special::Color(ColorSpecial::Pop));
        assert_eq!(
            parse(b"color gray 0.5"),
            Special::Color(ColorSpecial::Set(Color::Gray(0.5)))
        );
        assert_eq!(
            parse(b"color push Blue"),
            Special::Color(ColorSpecial::Push(Color::Cmyk(1.0, 1.0, 0.0, 0.0)))
        );
        assert_eq!(
            parse(b"background cmyk 0 0 0 0"),
            Special::Color(ColorSpecial::Background(Color::Cmyk(0.0, 0.0, 0.0, 0.0)))
        );
    }

    #[test]
    fn parses_papersize_xtx_and_unknown() {
        match parse(b"papersize=210mm,297mm") {
            Special::PaperSize(w, h) => assert!(
                (w - 210.0 * 72.0 / 25.4).abs() < 1e-9 && (h - 297.0 * 72.0 / 25.4).abs() < 1e-9
            ),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            parse(b"x:scale 2 3"),
            Special::Xtx(XtxSpecial::Scale(2.0, 3.0))
        );
        assert_eq!(parse(b"x:gsave"), Special::Xtx(XtxSpecial::GSave));
        assert!(matches!(parse(b"ps: gsave"), Special::Unsupported(_)));
        assert!(matches!(parse(b"src:12foo.tex"), Special::Unknown(_)));
        assert_eq!(
            parse(b"dvipdfmx:config C 0x10"),
            Special::Config("config C 0x10".into())
        );
        assert_eq!(
            parse(b"sabidvi:mark baseline"),
            Special::Mark("baseline".into())
        );
    }
}
