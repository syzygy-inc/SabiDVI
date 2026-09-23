//! TeX Live の各エンジンが出す DVI / XDV を実際に生成して解析する。エンジンが無ければ飛ばす。

use sabidvi_format::{Direction, Dvi, Kind, Op};
use std::path::PathBuf;
use std::process::Command;

fn run_engine(engine: &str, args: &[&str], name: &str, source: &str) -> Option<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("sabidvi-test-{}-{}", engine, std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let tex = dir.join(format!("{name}.tex"));
    std::fs::write(&tex, source).ok()?;
    let status = Command::new(engine)
        .args(args)
        .arg("-interaction=batchmode")
        .arg(format!("{name}.tex"))
        .current_dir(&dir)
        .output();
    let Ok(out) = status else {
        skip(&format!("{engine} not found"));
        return None;
    };
    let ext = if args.contains(&"-no-pdf") {
        "xdv"
    } else {
        "dvi"
    };
    let path: PathBuf = dir.join(format!("{name}.{ext}"));
    let data = std::fs::read(&path).ok();
    if data.is_none() {
        eprintln!(
            "{engine} produced no output: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
    data
}

#[test]
fn knuth_tex_dvi_parses_with_fonts_and_chars() {
    let Some(data) = run_engine(
        "tex",
        &[],
        "t",
        "\\font\\a=cmr10 \\a Hello\\special{x}\\vrule width 2pt\\bye\n",
    ) else {
        return;
    };
    let dvi = Dvi::parse(&data).unwrap();
    assert_eq!(dvi.kind, Kind::Dvi);
    assert_eq!(dvi.pages.len(), 2 - 1);
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
}

#[test]
fn uptex_tate_dvi_has_direction_changes() {
    let Some(data) = run_engine(
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
}

#[test]
fn xetex_xdv_has_native_fonts_and_glyphs() {
    let src = "\\font\\x=\"[lmroman10-regular.otf]\" at 10pt \\x Hello\\bye\n";
    let Some(data) = run_engine("xetex", &["-no-pdf"], "x", src) else {
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
}

/// 参照環境（TeX Live、フォント）が無いときは飛ばす。`SABI_STRICT_TESTS` が設定されていれば失敗にする。
/// 中核でない環境（upTeX、XeTeX、Times、Latin Modern、原ノ味）は `SABI_STRICT_OPTIONAL` も設定されているときだけ失敗にする
fn skip(reason: &str) {
    let optional = ["uptex", "xetex", "Times", "uprml", "Harano", ".otf"]
        .iter()
        .any(|k| reason.contains(k));
    let strict = std::env::var_os("SABI_STRICT_TESTS").is_some()
        && (!optional || std::env::var_os("SABI_STRICT_OPTIONAL").is_some());
    if strict {
        panic!("required reference environment is missing: {reason}");
    }
    eprintln!(
        "skipped{}: {reason}",
        if optional { " (optional)" } else { "" }
    );
}
