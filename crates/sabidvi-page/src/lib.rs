//! ページの実行。DVI の命令列を位置の計算（tex.web §585 の h, v, w, x, y, z とスタック）とともに辿り、
//! 文字・規則・special を SabiRender の描画命令列に変換する。
//!
//! 座標: DVI の (h, v) は sp、原点はページ左上から 1 in 内側、v は下向き。dvipdfmx と同じく紙面の左上から
//! (72 bp, 72 bp) の点を DVI の原点に置く。ページ空間は bp、原点左下、y 上向き。
//!
//! special の意味論は dvipdfmx（`spc_pdfm.c`）に従う:
//!
//! - `pdf:code`: 現在の図形状態のまま流す。座標は `bcontent` が積んだ原点から
//! - `pdf:literal`: 現在位置へ平行移動してから流し、戻す（`q`/`Q` は使わない）
//! - `pdf:bcontent`: `q` と現在位置への平行移動。原点スタックに積む。`econtent` で `Q` と色の再設定
//! - `pdf:btrans`: `q` と、現在位置を中心とする変換。`etrans` で `Q`
//! - `pdf:bcolor` / `ecolor` / `scolor`、dvips の `color push` / `pop`: 色スタック
//! - `pdf:bxobj` … `exobj`: 描画を捕まえて名前に結びつけ、`uxobj` で現在位置に置く
//! - `pdf:put @name << /K << /ca … >> >>`: 拡張図形状態の資源（PGF の不透明度）
//!
//! 文字は輪郭の供給源（[`GlyphSource`]）が返す輪郭で描く。返さない場合は数えて報告する。

use sabidvi_format::{Direction, Dvi, DviError, FontDef, Glyphs, NativeFontDef, Op};
use sabidvi_special::{ColorSpecial, DimTrans, PdfSpecial, Special, XtxSpecial};
use sabirender_content::lexer::{Lexer, Token};
use sabirender_content::{Evaluator, ExtGState, FormXObject, Resources};
use sabirender_display::{Color, DisplayList, FillRule, GlyphRun, Item, Matrix, Path, PlacedGlyph};
use std::cell::RefCell;
use std::collections::HashMap;

/// 紙面（bp）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paper {
    pub width: f64,
    pub height: f64,
}

impl Paper {
    pub const A4: Paper = Paper {
        width: 595.28,
        height: 841.89,
    };
    pub const LETTER: Paper = Paper {
        width: 612.0,
        height: 792.0,
    };
}

/// フォントの計量と字形の供給。TFM / JFM の読み取りは SabiFace に任せ、ここは結果だけを受け取る
pub trait FontSource {
    /// TFM フォントの文字幅（デザインサイズに対する比。TFM の `fix_word`）。無ければ None
    fn char_width(&self, font: &FontDef, code: u32) -> Option<f64>;
    /// 縦組の JFM で使う: 文字の高さ + 深さ（比）。横組では使わない
    fn char_vertical_advance(&self, font: &FontDef, code: u32) -> Option<f64> {
        let _ = (font, code);
        None
    }
    /// 字形の輪郭（字形単位）と、字形単位から em への比（1000 単位なら 0.001）
    fn glyph(&self, font: &FontDef, code: u32) -> Option<(Path, f64)>;
    /// XDV のネイティブフォントの字形
    fn native_glyph(&self, font: &NativeFontDef, glyph_id: u16) -> Option<(Path, f64)> {
        let _ = (font, glyph_id);
        None
    }
}

/// 何も供給しない（幅 0、輪郭なし）。構文の検証用
pub struct NoFonts;

impl FontSource for NoFonts {
    fn char_width(&self, _: &FontDef, _: u32) -> Option<f64> {
        None
    }
    fn glyph(&self, _: &FontDef, _: u32) -> Option<(Path, f64)> {
        None
    }
}

#[derive(Debug, Default, Clone)]
pub struct PageReport {
    pub missing_width: usize,
    pub missing_glyph: usize,
    pub chars: usize,
    pub rules: usize,
    pub specials: usize,
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct Position {
    h: i32,
    v: i32,
    w: i32,
    x: i32,
    y: i32,
    z: i32,
}

/// `pdf:put` で集めた資源と、`bxobj` で捕まえたフォーム
#[derive(Default)]
struct PageResources {
    ext_gstates: RefCell<HashMap<String, ExtGState>>,
}

impl Resources for PageResources {
    fn form_xobject(&self, _: &str) -> Option<FormXObject> {
        // フォームは `Do` ではなく uxobj で置くので、評価器からは見せない
        None
    }
    fn ext_gstate(&self, name: &str) -> Option<ExtGState> {
        self.ext_gstates.borrow().get(name).cloned()
    }
}

struct CapturedForm {
    /// 捕まえたときの現在位置（ページ空間）
    ref_x: f64,
    ref_y: f64,
    dims: DimTrans,
    items: Vec<Item>,
}

pub struct PageExecutor<'d, 'f> {
    dvi: &'d Dvi<'d>,
    fonts: &'f dyn FontSource,
    pub paper: Paper,
    bp_per_sp: f64,
}

impl<'d, 'f> PageExecutor<'d, 'f> {
    pub fn new(dvi: &'d Dvi<'d>, fonts: &'f dyn FontSource, paper: Paper) -> Self {
        PageExecutor {
            dvi,
            fonts,
            paper,
            bp_per_sp: dvi.bp_per_sp(),
        }
    }

    /// DVI の (h, v)（sp）をページ空間（bp）へ
    pub fn to_page(&self, h: i32, v: i32) -> (f64, f64) {
        (
            72.0 + h as f64 * self.bp_per_sp,
            self.paper.height - 72.0 - v as f64 * self.bp_per_sp,
        )
    }

    pub fn run_page(&self, index: usize) -> Result<(DisplayList, PageReport), DviError> {
        let ops = self.dvi.page_ops(index)?;
        let mut out = DisplayList::default();
        let mut report = PageReport::default();
        let resources = PageResources::default();
        let mut ev = Evaluator::new(Matrix::IDENTITY, &resources);
        let mut pos = Position {
            h: 0,
            v: 0,
            w: 0,
            x: 0,
            y: 0,
            z: 0,
        };
        let mut stack: Vec<Position> = Vec::new();
        let mut dir = Direction::Yoko;
        let mut font: Option<u32> = None;
        // special の状態
        let mut coord_stack: Vec<(f64, f64)> = Vec::new();
        let mut color_stack: Vec<(Color, Color)> = Vec::new();
        let mut forms: HashMap<String, CapturedForm> = HashMap::new();
        let mut capturing: Option<(String, CapturedForm, usize)> = None;

        let sp = self.bp_per_sp;
        for op in &ops {
            match op {
                Op::SetChar(c) | Op::PutChar(c) => {
                    report.chars += 1;
                    let advance =
                        self.draw_char(font, *c, &pos, &dir, &mut ev, &mut out, &mut report);
                    if matches!(op, Op::SetChar(_)) {
                        advance_right(&mut pos, &dir, advance);
                    }
                }
                Op::SetRule { height, width } | Op::PutRule { height, width } => {
                    report.rules += 1;
                    if *height > 0 && *width > 0 {
                        let (a, b) = (*height as f64 * sp, *width as f64 * sp);
                        let (x, y) = self.to_page(pos.h, pos.v);
                        let path = match dir {
                            Direction::Yoko => Path::rect(x, y, b, a),
                            // 縦組: 高さ a が左向き、幅 b が下向き
                            Direction::Tate => Path::rect(x - a, y - b, a, b),
                            Direction::Dtou => Path::rect(x, y, a, b),
                        };
                        out.push(Item::Fill {
                            path,
                            ctm: Matrix::IDENTITY,
                            rule: FillRule::NonZero,
                            color: ev.state.fill_color,
                            alpha: ev.state.fill_alpha,
                        });
                    }
                    if matches!(op, Op::SetRule { .. }) {
                        advance_right(&mut pos, &dir, *width);
                    }
                }
                Op::SetGlyphs(g) => {
                    report.chars += g.ids.len();
                    self.draw_native_glyphs(font, g, &pos, &mut ev, &mut out, &mut report);
                    advance_right(&mut pos, &dir, g.width);
                }
                Op::Nop
                | Op::Bop { .. }
                | Op::Eop
                | Op::FontDef(_)
                | Op::NativeFontDef(_)
                | Op::Pre { .. }
                | Op::Post
                | Op::PostPost => {}
                Op::Push => stack.push(pos),
                Op::Pop => {
                    if let Some(p) = stack.pop() {
                        pos = p;
                    }
                }
                Op::Right(d) => advance_right(&mut pos, &dir, *d),
                Op::W0 => {
                    let d = pos.w;
                    advance_right(&mut pos, &dir, d);
                }
                Op::W(d) => {
                    pos.w = *d;
                    advance_right(&mut pos, &dir, *d);
                }
                Op::X0 => {
                    let d = pos.x;
                    advance_right(&mut pos, &dir, d);
                }
                Op::X(d) => {
                    pos.x = *d;
                    advance_right(&mut pos, &dir, *d);
                }
                Op::Down(d) => advance_down(&mut pos, &dir, *d),
                Op::Y0 => {
                    let d = pos.y;
                    advance_down(&mut pos, &dir, d);
                }
                Op::Y(d) => {
                    pos.y = *d;
                    advance_down(&mut pos, &dir, *d);
                }
                Op::Z0 => {
                    let d = pos.z;
                    advance_down(&mut pos, &dir, d);
                }
                Op::Z(d) => {
                    pos.z = *d;
                    advance_down(&mut pos, &dir, *d);
                }
                Op::Font(k) => font = Some(*k),
                Op::Dir(d) => dir = d.clone(),
                Op::PicFile { path, .. } => {
                    report
                        .unsupported
                        .push(format!("pic_file {}", String::from_utf8_lossy(path)));
                }
                Op::Special(raw) => {
                    report.specials += 1;
                    let (x_user, y_user) = self.to_page(pos.h, pos.v);
                    let (ox, oy) = coord_stack.last().copied().unwrap_or((0.0, 0.0));
                    // dvipdfmx の spc_get_current_point: bcontent の原点からの相対位置
                    let (cx, cy) = (x_user - ox, y_user - oy);
                    let target: &mut DisplayList = &mut out;
                    match sabidvi_special::parse(raw) {
                        Special::Pdf(p) => match p {
                            PdfSpecial::Code(code) => ev.run(code.as_bytes(), target),
                            PdfSpecial::Literal { direct, code } => {
                                if direct {
                                    ev.run(code.as_bytes(), target);
                                } else {
                                    ev.run(
                                        format!("1 0 0 1 {} {} cm", fmt(cx), fmt(cy)).as_bytes(),
                                        target,
                                    );
                                    ev.run(code.as_bytes(), target);
                                    ev.run(
                                        format!("1 0 0 1 {} {} cm", fmt(-cx), fmt(-cy)).as_bytes(),
                                        target,
                                    );
                                }
                            }
                            PdfSpecial::BeginContent => {
                                ev.run(
                                    format!("q 1 0 0 1 {} {} cm", fmt(cx), fmt(cy)).as_bytes(),
                                    target,
                                );
                                coord_stack.push((x_user, y_user));
                            }
                            PdfSpecial::EndContent => {
                                coord_stack.pop();
                                ev.run(b"Q", target);
                                reset_color(&mut ev, &color_stack);
                            }
                            PdfSpecial::BeginTransform(t) => {
                                let m = t.matrix;
                                // 現在位置を中心とする変換（spc_handler_pdfm_btrans）
                                let e = m.e + (1.0 - m.a) * cx - m.c * cy;
                                let f = m.f + (1.0 - m.d) * cy - m.b * cx;
                                ev.run(
                                    format!(
                                        "q {} {} {} {} {} {} cm",
                                        fmt(m.a),
                                        fmt(m.b),
                                        fmt(m.c),
                                        fmt(m.d),
                                        fmt(e),
                                        fmt(f)
                                    )
                                    .as_bytes(),
                                    target,
                                );
                            }
                            PdfSpecial::EndTransform => {
                                ev.run(b"Q", target);
                                reset_color(&mut ev, &color_stack);
                            }
                            PdfSpecial::BeginColor { fill, stroke } => {
                                color_stack.push((ev.state.stroke_color, ev.state.fill_color));
                                apply_color(&mut ev, fill, stroke);
                            }
                            PdfSpecial::SetColor { fill, stroke } => {
                                apply_color(&mut ev, fill, stroke);
                                if let Some(top) = color_stack.last_mut() {
                                    *top = (ev.state.stroke_color, ev.state.fill_color);
                                }
                            }
                            PdfSpecial::EndColor => {
                                if let Some((s, f)) = color_stack.pop() {
                                    ev.state.stroke_color = s;
                                    ev.state.fill_color = f;
                                }
                            }
                            PdfSpecial::BeginXObject { ident, dims } => {
                                if capturing.is_some() {
                                    report.unsupported.push("nested bxobj".into());
                                } else {
                                    let start = out.items.len();
                                    capturing = Some((
                                        ident,
                                        CapturedForm {
                                            ref_x: x_user,
                                            ref_y: y_user,
                                            dims,
                                            items: Vec::new(),
                                        },
                                        start,
                                    ));
                                }
                            }
                            PdfSpecial::EndXObject => match capturing.take() {
                                Some((ident, mut form, start)) => {
                                    form.items = out.items.drain(start..).collect();
                                    forms.insert(ident, form);
                                }
                                None => report.unsupported.push("exobj without bxobj".into()),
                            },
                            PdfSpecial::UseXObject { ident, dims } => match forms.get(&ident) {
                                Some(form) => {
                                    let m = placement_matrix(form, &dims, x_user, y_user);
                                    for item in &form.items {
                                        out.push(retarget(item, &m));
                                    }
                                }
                                None => report
                                    .unsupported
                                    .push(format!("uxobj {ident} (undefined)")),
                            },
                            PdfSpecial::Image { file, .. } => {
                                report.unsupported.push(format!("image {file}"))
                            }
                            PdfSpecial::Put { ident, body }
                            | PdfSpecial::Object { ident, body } => {
                                collect_ext_gstates(&body, &resources);
                                let _ = ident;
                            }
                            PdfSpecial::Stream { .. } => {
                                report.unsupported.push("pdf:stream".into())
                            }
                            PdfSpecial::PageSize(_) => {}
                            PdfSpecial::Ignored(_) => {}
                            PdfSpecial::Unsupported(k) => {
                                report.unsupported.push(format!("pdf:{k}"))
                            }
                            PdfSpecial::Unknown(k) => report.unsupported.push(format!("pdf:{k}")),
                        },
                        Special::Color(c) => match c {
                            ColorSpecial::Push(col) => {
                                color_stack.push((ev.state.stroke_color, ev.state.fill_color));
                                apply_color(&mut ev, Some(col), Some(col));
                            }
                            ColorSpecial::Pop => {
                                if let Some((s, f)) = color_stack.pop() {
                                    ev.state.stroke_color = s;
                                    ev.state.fill_color = f;
                                }
                            }
                            ColorSpecial::Set(col) => apply_color(&mut ev, Some(col), Some(col)),
                            ColorSpecial::Background(_) => {}
                        },
                        Special::Xtx(x) => match x {
                            XtxSpecial::GSave => ev.run(b"q", target),
                            XtxSpecial::GRestore => ev.run(b"Q", target),
                            XtxSpecial::Scale(sx, sy) => ev.run(
                                format!(
                                    "1 0 0 1 {} {} cm {} 0 0 {} 0 0 cm 1 0 0 1 {} {} cm",
                                    fmt(cx),
                                    fmt(cy),
                                    fmt(sx),
                                    fmt(sy),
                                    fmt(-cx),
                                    fmt(-cy)
                                )
                                .as_bytes(),
                                target,
                            ),
                            XtxSpecial::Rotate(deg) => {
                                let (s, c) = (deg.to_radians().sin(), deg.to_radians().cos());
                                ev.run(
                                    format!(
                                        "1 0 0 1 {} {} cm {} {} {} {} 0 0 cm 1 0 0 1 {} {} cm",
                                        fmt(cx),
                                        fmt(cy),
                                        fmt(c),
                                        fmt(s),
                                        fmt(-s),
                                        fmt(c),
                                        fmt(-cx),
                                        fmt(-cy)
                                    )
                                    .as_bytes(),
                                    target,
                                )
                            }
                            XtxSpecial::Ignored(_) => {}
                        },
                        Special::PaperSize(..) | Special::Landscape | Special::Config(_) => {}
                        Special::Unsupported(s) => report.unsupported.push(s),
                        Special::Unknown(s) => {
                            report.unsupported.push(format!("unknown special: {s}"))
                        }
                    }
                }
            }
        }
        ev.finish(&mut out);
        report
            .unsupported
            .extend(out.unsupported().map(|s| s.to_string()));
        Ok((out, report))
    }

    /// 文字を描き、送り幅（sp）を返す
    #[allow(clippy::too_many_arguments)]
    fn draw_char(
        &self,
        font: Option<u32>,
        code: u32,
        pos: &Position,
        dir: &Direction,
        ev: &mut Evaluator,
        out: &mut DisplayList,
        report: &mut PageReport,
    ) -> i32 {
        let Some(def) = font.and_then(|k| self.dvi.font(k)) else {
            report.missing_width += 1;
            return 0;
        };
        let size = def.scaled_size as f64;
        let advance = match dir {
            Direction::Yoko => self.fonts.char_width(def, code),
            _ => self
                .fonts
                .char_vertical_advance(def, code)
                .or_else(|| self.fonts.char_width(def, code)),
        };
        let advance_sp = match advance {
            Some(w) => (w * size).round() as i32,
            None => {
                report.missing_width += 1;
                0
            }
        };
        match self.fonts.glyph(def, code) {
            Some((outline, units_to_em)) => {
                let (x, y) = self.to_page(pos.h, pos.v);
                let scale = units_to_em * size * self.bp_per_sp;
                let ctm = match dir {
                    Direction::Yoko => Matrix::IDENTITY,
                    // 縦組: 字形を時計回りに 90° 回して、文字の中心線を現在位置に合わせる（近似）
                    Direction::Tate => Matrix::new(0.0, -1.0, 1.0, 0.0, x - y * 1.0, y + x * 1.0)
                        .then(&Matrix::IDENTITY),
                    Direction::Dtou => Matrix::IDENTITY,
                };
                let run = GlyphRun {
                    glyphs: vec![PlacedGlyph { outline, x, y }],
                    scale,
                };
                out.push(Item::Glyphs {
                    run,
                    ctm,
                    color: ev.state.fill_color,
                    alpha: ev.state.fill_alpha,
                });
            }
            None => report.missing_glyph += 1,
        }
        advance_sp
    }

    fn draw_native_glyphs(
        &self,
        font: Option<u32>,
        g: &Glyphs,
        pos: &Position,
        ev: &mut Evaluator,
        out: &mut DisplayList,
        report: &mut PageReport,
    ) {
        let Some(def) = font.and_then(|k| self.dvi.native_font(k)) else {
            report.missing_glyph += g.ids.len();
            return;
        };
        let mut glyphs = Vec::new();
        let mut scale = 0.0;
        for (i, id) in g.ids.iter().enumerate() {
            match self.fonts.native_glyph(def, *id) {
                Some((outline, units_to_em)) => {
                    let (dx, dy) = g.positions[i];
                    let (x, y) = self.to_page(pos.h + dx, pos.v + dy);
                    scale = units_to_em * def.size as f64 * self.bp_per_sp;
                    glyphs.push(PlacedGlyph { outline, x, y });
                }
                None => report.missing_glyph += 1,
            }
        }
        if !glyphs.is_empty() {
            let color = match def.rgba {
                Some([r, g, b, _]) => {
                    Color::Rgb(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
                }
                None => ev.state.fill_color,
            };
            out.push(Item::Glyphs {
                run: GlyphRun { glyphs, scale },
                ctm: Matrix::IDENTITY,
                color,
                alpha: ev.state.fill_alpha,
            });
        }
    }
}

fn advance_right(pos: &mut Position, dir: &Direction, d: i32) {
    match dir {
        Direction::Yoko => pos.h += d,
        Direction::Tate => pos.v += d,
        Direction::Dtou => pos.v -= d,
    }
}

fn advance_down(pos: &mut Position, dir: &Direction, d: i32) {
    match dir {
        Direction::Yoko => pos.v += d,
        Direction::Tate => pos.h -= d,
        Direction::Dtou => pos.h += d,
    }
}

fn fmt(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s
    }
}

fn apply_color(ev: &mut Evaluator, fill: Option<Color>, stroke: Option<Color>) {
    if let Some(f) = fill {
        ev.state.fill_color = f;
    }
    if let Some(s) = stroke {
        ev.state.stroke_color = s;
    }
}

/// `Q` の後に色スタックの先頭の色を再設定する（dvipdfmx の `pdf_dev_reset_color`）
fn reset_color(ev: &mut Evaluator, color_stack: &[(Color, Color)]) {
    if let Some((s, f)) = color_stack.last() {
        ev.state.stroke_color = *s;
        ev.state.fill_color = *f;
    }
}

/// フォームを現在位置に置く行列。参照点を現在位置へ平行移動し、width / height があれば境界箱に合わせて拡大する
fn placement_matrix(form: &CapturedForm, dims: &DimTrans, x_user: f64, y_user: f64) -> Matrix {
    let bbox = form.dims.bbox.unwrap_or_else(|| {
        [
            0.0,
            -form.dims.depth.unwrap_or(0.0),
            form.dims.width.unwrap_or(1.0),
            form.dims.height.unwrap_or(1.0),
        ]
    });
    let bw = (bbox[2] - bbox[0]).abs().max(1e-9);
    let bh = (bbox[3] - bbox[1]).abs().max(1e-9);
    let (sx, sy) = match (dims.width, dims.height) {
        (Some(w), Some(h)) => (w / bw, h / bh),
        (Some(w), None) => (w / bw, w / bw),
        (None, Some(h)) => (h / bh, h / bh),
        (None, None) => (1.0, 1.0),
    };
    Matrix::translate(-form.ref_x, -form.ref_y)
        .then(&Matrix::scale(sx, sy))
        .then(&dims.matrix)
        .then(&Matrix::translate(x_user, y_user))
}

fn retarget(item: &Item, m: &Matrix) -> Item {
    match item {
        Item::Fill {
            path,
            ctm,
            rule,
            color,
            alpha,
        } => Item::Fill {
            path: path.clone(),
            ctm: ctm.then(m),
            rule: *rule,
            color: *color,
            alpha: *alpha,
        },
        Item::Stroke {
            path,
            ctm,
            style,
            color,
            alpha,
        } => Item::Stroke {
            path: path.clone(),
            ctm: ctm.then(m),
            style: style.clone(),
            color: *color,
            alpha: *alpha,
        },
        Item::ClipPush { path, ctm, rule } => Item::ClipPush {
            path: path.clone(),
            ctm: ctm.then(m),
            rule: *rule,
        },
        Item::ClipPop => Item::ClipPop,
        Item::Glyphs {
            run,
            ctm,
            color,
            alpha,
        } => Item::Glyphs {
            run: run.clone(),
            ctm: ctm.then(m),
            color: *color,
            alpha: *alpha,
        },
        Item::Image {
            width,
            height,
            rgba,
            ctm,
            alpha,
        } => Item::Image {
            width: *width,
            height: *height,
            rgba: rgba.clone(),
            ctm: ctm.then(m),
            alpha: *alpha,
        },
        Item::Unsupported { what } => Item::Unsupported { what: what.clone() },
    }
}

/// `<< /name << /ca 0.5 /CA 0.5 /LW 2 >> … >>` から拡張図形状態を集める
fn collect_ext_gstates(body: &str, resources: &PageResources) {
    let mut lx = Lexer::new(body.as_bytes());
    let mut tokens = Vec::new();
    while let Some(t) = lx.next_token() {
        tokens.push(t);
    }
    // 深さ 1 の名前の直後に辞書が来るものを拾う
    let mut depth = 0i32;
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::DictOpen => depth += 1,
            Token::DictClose => depth -= 1,
            Token::Name(name)
                if depth == 1 && matches!(tokens.get(i + 1), Some(Token::DictOpen)) =>
            {
                let mut g = ExtGState::default();
                let mut j = i + 2;
                let mut d = 1;
                while j < tokens.len() && d > 0 {
                    match &tokens[j] {
                        Token::DictOpen => d += 1,
                        Token::DictClose => d -= 1,
                        Token::Name(k) if d == 1 => {
                            if let Some(Token::Number(v)) = tokens.get(j + 1) {
                                match k.as_str() {
                                    "ca" => g.fill_alpha = Some(*v),
                                    "CA" => g.stroke_alpha = Some(*v),
                                    "LW" => g.line_width = Some(*v),
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                if g != ExtGState::default() {
                    resources.ext_gstates.borrow_mut().insert(name.clone(), g);
                }
                i = j;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_gstate_dictionaries_are_collected_from_put() {
        let r = PageResources::default();
        collect_ext_gstates(
            "<< /pgfgs1 << /ca 0.5 >> /pgfgs2 << /CA 0.25 /LW 3 >> >>",
            &r,
        );
        let m = r.ext_gstates.borrow();
        assert_eq!(m["pgfgs1"].fill_alpha, Some(0.5));
        assert_eq!(m["pgfgs2"].stroke_alpha, Some(0.25));
        assert_eq!(m["pgfgs2"].line_width, Some(3.0));
    }

    #[test]
    fn numbers_are_formatted_compactly() {
        assert_eq!(fmt(1.0), "1");
        assert_eq!(fmt(-0.5), "-0.5");
        assert_eq!(fmt(0.0), "0");
        assert_eq!(fmt(72.27), "72.27");
    }
}
