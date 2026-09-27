//! TeX Live の各エンジンが出す DVI / XDV を実際に生成して解析する。
//! 各テストは契約 case（`specification/cases.md`）。エンジンが無ければ BLOCKED、起動したのに DVI を出さなければ FAIL。

use sabidvi_format::{Direction, Dvi, Kind, Op};
use sabidvi_qa::{run_tool, Case};

fn run_engine(
    case: &Case,
    engine: &str,
    args: &[&str],
    name: &str,
    source: &str,
) -> Option<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("sabidvi-test-{}-{}", engine, std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.tex")), source).unwrap();
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
    let path = dir.join(format!("{name}.{ext}"));
    let data = std::fs::read(&path);
    let _ = std::fs::remove_dir_all(&dir);
    match data {
        Ok(d) => Some(d),
        Err(_) => case.tool_failed(&format!("{engine} produced no {ext}")),
    }
}

/// DVI-ENGINE-TEX
#[test]
fn knuth_tex_dvi_parses_with_fonts_and_chars() {
    let case = Case::required("DVI-ENGINE-TEX", &["C-DVI"]);
    let Some(data) = run_engine(
        &case,
        "tex",
        &[],
        "t",
        "\\font\\a=cmr10 \\a Hello\\special{x}\\vrule width 2pt\\bye\n",
    ) else {
        return;
    };
    let dvi = Dvi::parse(&data).unwrap();
    assert_eq!(dvi.kind, Kind::Dvi);
    assert_eq!(dvi.pages.len(), 1);
    assert!(dvi.fonts.iter().any(|f| f.name == "cmr10"));
    let ops = dvi.page_ops(0).unwrap();
    let chars: Vec<u32> = ops
        .iter()
        .filter_map(|o| {
            if let Op::SetChar(c) = o {
                Some(*c)
            } else {
                None
            }
        })
        .collect();
    assert!(
        chars.starts_with(&[
            b'H' as u32,
            b'e' as u32,
            b'l' as u32,
            b'l' as u32,
            b'o' as u32
        ]),
        "{chars:?}"
    );
    assert!(ops.contains(&Op::Special(b"x".to_vec())));
    assert!(ops.iter().any(|o| matches!(o, Op::SetRule { .. })));
    assert!(!dvi.has_tate);
    case.compared_n(6);
    case.done();
}

/// DVI-ENGINE-UPTEX（任意: texlive-lang-japanese）
#[test]
fn uptex_tate_dvi_has_direction_changes() {
    let case = Case::optional("DVI-ENGINE-UPTEX", &["C-DVI"]);
    let Some(data) = run_engine(
        &case,
        "uptex",
        &[],
        "u",
        "\\tate\\font\\y=upjisr-v \\y \\hbox{\\yoko A}\\bye\n",
    ) else {
        return;
    };
    let dvi = Dvi::parse(&data).unwrap();
    assert_eq!(dvi.kind, Kind::Dvi);
    assert!(dvi.has_tate, "post_post id should be 3 for tate");
    let ops = dvi.page_ops(0).unwrap();
    let dirs: Vec<&Direction> = ops
        .iter()
        .filter_map(|o| if let Op::Dir(d) = o { Some(d) } else { None })
        .collect();
    assert!(
        dirs.contains(&&Direction::Tate) && dirs.contains(&&Direction::Yoko),
        "{dirs:?}"
    );
    case.compared_n(3);
    case.done();
}

/// DVI-ENGINE-XETEX（任意: xetex と Latin Modern の OpenType）
#[test]
fn xetex_xdv_has_native_fonts_and_glyphs() {
    let case = Case::optional("DVI-ENGINE-XETEX", &["C-DVI"]);
    let src = "\\font\\x=\"[lmroman10-regular.otf]\" at 10pt \\x Hello\\bye\n";
    let Some(data) = run_engine(&case, "xetex", &["-no-pdf"], "x", src) else {
        return;
    };
    let dvi = Dvi::parse(&data).unwrap();
    assert_eq!(dvi.kind, Kind::Xdv);
    assert_eq!(dvi.native_fonts.len(), 1);
    let f = &dvi.native_fonts[0];
    assert!(f.name.contains("lmroman10-regular.otf"), "{}", f.name);
    assert_eq!(f.size, 10 * 65536);
    assert!(!f.is_vertical());
    let ops = dvi.page_ops(0).unwrap();
    let glyphs = ops
        .iter()
        .find_map(|o| {
            if let Op::SetGlyphs(g) = o {
                Some(g)
            } else {
                None
            }
        })
        .expect("set_glyphs");
    assert_eq!(glyphs.ids.len(), 5);
    assert_eq!(glyphs.positions.len(), 5);
    assert!(glyphs.width > 0);
    case.compared_n(8);
    case.done();
}
