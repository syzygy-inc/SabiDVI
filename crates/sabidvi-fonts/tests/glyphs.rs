//! 字形の供給を、TeX Live のエンジンで作った DVI / XDV を実際に描いて確かめる。
//! 各テストは契約 case（`specification/cases.md`）。環境不足は BLOCKED、エンジンの失敗は FAIL。

use sabidvi_fonts::{Kpse, MemoryLocator, SabiFonts};
use sabidvi_format::{Dvi, FontDef};
use sabidvi_page::{FontSource, PageExecutor, Paper};
use sabidvi_qa::{kpsewhich, run_tool, Case};
use sabirender_display::Item;
use sabirender_raster::{page_to_device, render, Canvas};

fn run(case: &Case, engine: &str, args: &[&str], name: &str, src: &str) -> Option<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("sabidvi-fonts-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.tex")), src).unwrap();
    let mut full: Vec<&str> = args.to_vec();
    full.push("-interaction=batchmode");
    let file = format!("{name}.tex");
    full.push(&file);
    if run_tool(case, engine, &full, Some(&dir)).is_none() {
        case.blocked(&format!("{engine} not found"));
        return None;
    }
    let ext = if args.contains(&"-no-pdf") {
        "xdv"
    } else {
        "dvi"
    };
    let data = std::fs::read(dir.join(format!("{name}.{ext}")));
    let _ = std::fs::remove_dir_all(&dir);
    match data {
        Ok(d) => Some(d),
        Err(_) => case.tool_failed(&format!("{engine} produced no {ext}")),
    }
}

/// ページを 100 dpi で描き、インクのある画素の境界箱（画素座標）を返す。`max_y` より下（ページ番号など）は数えない
fn ink_bbox(
    data: &[u8],
    fonts: &SabiFonts<Kpse>,
    max_y: usize,
) -> (usize, usize, usize, usize, sabidvi_page::PageReport) {
    let dvi = Dvi::parse(data).unwrap();
    let exec = PageExecutor::new(&dvi, fonts, Paper::A4);
    let (list, report) = exec.run_page(0).unwrap();
    let dpi = 100.0;
    let (w, h) = (
        (Paper::A4.width * dpi / 72.0).ceil() as usize,
        (Paper::A4.height * dpi / 72.0).ceil() as usize,
    );
    let mut canvas = Canvas::new(w, h);
    render(&list, &mut canvas, &page_to_device(Paper::A4.height, dpi));
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..h.min(max_y) {
        for x in 0..w {
            if canvas.pixels[y * w + x][3] > 0.5 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    assert!(x1 > 0, "no ink");
    (x0, y0, x1, y1, report)
}

/// DVI-GLYPH-CM: Computer Modern の Type1 字形（tex、cmr10）
#[test]
fn computer_modern_type1_glyphs_render_on_the_first_line() {
    let case = Case::required("DVI-GLYPH-CM", &["C-FONT", "C-DVI"]);
    let Some(data) = run(&case, "tex", &[], "cm", "\\font\\a=cmr10 \\a Hello\\bye\n") else {
        return;
    };
    let fonts = SabiFonts::new(Kpse::default());
    let dvi = Dvi::parse(&data).unwrap();
    let exec = PageExecutor::new(&dvi, &fonts, Paper::A4);
    let (list, report) = exec.run_page(0).unwrap();
    assert_eq!(report.missing_glyph, 0, "{report:?}");
    let glyphs = list
        .items
        .iter()
        .filter(|i| matches!(i, Item::Glyphs { .. }))
        .count();
    assert_eq!(glyphs, 6, "H e l l o and the page number");
    let (x0, y0, x1, y1, _) = ink_bbox(&data, &fonts, usize::MAX);
    // 1 行目: 上端 1in + 20pt の字下げ。100 dpi で x は 100 + 27.7 付近から、y は 100 付近
    assert!((125..=135).contains(&x0), "x0 = {x0}");
    assert!((95..=115).contains(&y0), "y0 = {y0}");
    // "Hello" は 10pt で幅 2.4em ≈ 33 画素。ページ番号は下端付近
    assert!(x1 > x0 + 25 && x1 < x0 + 300, "x1 = {x1}");
    assert!(y1 > 1000, "page number near the bottom: y1 = {y1}");
    case.compared_n(6);
    case.done();
}

/// DVI-GLYPH-TIMES（任意: URW の Times）: 仮想フォントの合成
#[test]
fn virtual_font_glyphs_are_composed_from_type1_parts() {
    let case = Case::optional("DVI-GLYPH-TIMES", &["C-FONT"]);
    let Some(data) = run(&case, "tex", &[], "vf", "\\font\\t=ptmr7t \\t AV\\bye\n") else {
        return;
    };
    let fonts = SabiFonts::new(Kpse::default());
    if fonts
        .map()
        .get("ptmr8r")
        .and_then(|e| e.font_file.clone())
        .and_then(|f| Kpse::default().find(&f))
        .is_none()
    {
        return case.blocked("Times (utmr8a.pfb) not installed");
    }
    let dvi = Dvi::parse(&data).unwrap();
    let exec = PageExecutor::new(&dvi, &fonts, Paper::A4);
    let (list, report) = exec.run_page(0).unwrap();
    assert_eq!(report.missing_glyph, 0, "{report:?}");
    let first = list
        .items
        .iter()
        .find_map(|i| {
            if let Item::Glyphs { run, .. } = i {
                Some(run)
            } else {
                None
            }
        })
        .unwrap();
    assert!(!first.glyphs[0].outline.is_empty());
    // A の輪郭は 1000/em 単位で幅 700 程度
    let xs: Vec<f64> = first.glyphs[0]
        .outline
        .segments
        .iter()
        .filter_map(|s| match *s {
            sabirender_display::Segment::MoveTo(x, _)
            | sabirender_display::Segment::LineTo(x, _) => Some(x),
            sabirender_display::Segment::CurveTo(_, _, _, _, x, _) => Some(x),
            _ => None,
        })
        .collect();
    let width =
        xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min);
    assert!((500.0..900.0).contains(&width), "A width {width}");
    // 字形単位 (1000/em) → bp: 10pt のフォントなので 0.001 × 10 × 72/72.27
    let m = first.glyphs[0].transform;
    assert!((m.a - 0.001 * 10.0 * 72.0 / 72.27).abs() < 1e-9 && m.b.abs() < 1e-12);
    case.compared_n(4);
    case.done();
}

/// DVI-GLYPH-XETEX（任意: xetex、Latin Modern の OpenType）
#[test]
fn xetex_native_font_glyphs_render() {
    let case = Case::optional("DVI-GLYPH-XETEX", &["C-FONT", "C-DVI"]);
    let Some(data) = run(
        &case,
        "xetex",
        &["-no-pdf"],
        "xdv",
        "\\font\\x=\"[lmroman10-regular.otf]\" at 10pt \\x Hello\\bye\n",
    ) else {
        return;
    };
    let fonts = SabiFonts::new(Kpse::default());
    let (x0, y0, _, _, report) = ink_bbox(&data, &fonts, 400);
    assert_eq!(report.missing_glyph, 0, "{report:?}");
    assert!((125..=135).contains(&x0), "x0 = {x0}");
    assert!((95..=115).contains(&y0), "y0 = {y0}");
    case.compared_n(3);
    case.done();
}

/// DVI-GLYPH-UPTEX（任意: uptex、原ノ味）: upjisr-h は仮想フォントで、部品は kanjix.map の uprml-h（OpenType）
#[test]
fn uptex_japanese_glyphs_come_through_vf_and_kanjix_map() {
    let case = Case::optional("DVI-GLYPH-UPTEX", &["C-FONT", "C-DVI"]);
    let Some(data) = run(
        &case,
        "uptex",
        &[],
        "up",
        "\\font\\j=upjisr-h \\j 漢字\\bye\n",
    ) else {
        return;
    };
    let fonts = SabiFonts::new(Kpse::default());
    let Some(entry) = fonts.map().get_kanji("uprml-h").cloned() else {
        return case.blocked("uprml-h not in kanjix.map");
    };
    if Kpse::default()
        .find(
            entry
                .font_file
                .trim_start_matches(|c| c == ':' || char::is_numeric(c)),
        )
        .is_none()
    {
        return case.blocked(&format!("{} not installed", entry.font_file));
    }
    let (x0, _, x1, _, report) = ink_bbox(&data, &fonts, 400);
    assert_eq!(report.missing_glyph, 0, "{report:?}");
    // 全角 2 文字 = 2 × 10pt ≈ 27.7 画素
    assert!((20..=40).contains(&(x1 - x0)), "width {}", x1 - x0);
    case.compared_n(2);
    case.done();
}

/// DVI-FONT-OTMAP（任意: Latin Modern の OpenType）: pdftex.map の SlantFont / ExtendFont は OpenType にも効く
#[test]
fn opentype_map_entries_apply_slant_and_extend() {
    let case = Case::optional("DVI-FONT-OTMAP", &["C-FONT"]);
    let Some(otf) = kpsewhich("lmroman10-regular.otf") else {
        return case.blocked("lmroman10-regular.otf not found");
    };
    let Some(tfm) = kpsewhich("cmr10.tfm") else {
        return case.blocked("cmr10.tfm not found");
    };
    let mut files = MemoryLocator::default();
    files.add("lmroman10-regular.otf", std::fs::read(otf).unwrap());
    files.add("xfont.tfm", std::fs::read(tfm).unwrap());
    files.add(
        "pdftex.map",
        b"xfont LMRoman10-Regular \".25 SlantFont 1.5 ExtendFont\" <<lmroman10-regular.otf\n"
            .to_vec(),
    );
    let fonts = SabiFonts::new(files);
    let def = FontDef {
        number: 0,
        checksum: 0,
        scaled_size: 655360,
        design_size: 655360,
        area: String::new(),
        name: "xfont".into(),
    };
    let (path, m) = fonts.glyph(&def, b'A' as u32).expect("glyph A");
    assert!(!path.is_empty());
    // 字形単位 → em は 1/unitsPerEm (1000) に [1.5 0 .25 1 0 0] を掛けたもの
    assert!((m.a - 0.0015).abs() < 1e-12, "a = {}", m.a);
    assert!((m.c - 0.00025).abs() < 1e-12, "c = {}", m.c);
    assert!((m.d - 0.001).abs() < 1e-12, "d = {}", m.d);
    assert!(m.b.abs() < 1e-12);
    case.compared_n(5);
    case.done();
}
