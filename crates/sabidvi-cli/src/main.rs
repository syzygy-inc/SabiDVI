//! `sabidvi render file.dvi [-o out.png] [--page N] [--dpi D] [--paper a4|letter]`
//!
//! DVI / XDV の 1 ページを SabiRender の参照ラスタライザで PNG にする。フォントは kpsewhich で探す。

use sabidvi_fonts::{Kpse, SabiFonts};
use sabidvi_format::Dvi;
use sabidvi_page::{PageExecutor, Paper};
use sabirender_raster::{page_to_device, png, render, Canvas};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|s| s.as_str()) != Some("render") || args.len() < 2 {
        eprintln!("usage: sabidvi render <file.dvi|file.xdv> [-o out.png] [--page N] [--dpi D] [--paper a4|letter]");
        std::process::exit(2);
    }
    let input = &args[1];
    let mut out = "out.png".to_string();
    let mut page = 1usize;
    let mut dpi = 150.0;
    let mut paper = Paper::A4;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                out = args.get(i + 1).cloned().unwrap_or(out);
                i += 2;
            }
            "--page" => {
                page = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(1);
                i += 2;
            }
            "--dpi" => {
                dpi = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(150.0);
                i += 2;
            }
            "--paper" => {
                paper = match args.get(i + 1).map(|s| s.as_str()) {
                    Some("letter") => Paper::LETTER,
                    _ => Paper::A4,
                };
                i += 2;
            }
            other => {
                eprintln!("unknown option {other}");
                std::process::exit(2);
            }
        }
    }
    let data = match std::fs::read(input) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{input}: {e}");
            std::process::exit(1);
        }
    };
    let dvi = match Dvi::parse(&data) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    if page == 0 || page > dvi.pages.len() {
        eprintln!("page {page} out of range (1..={})", dvi.pages.len());
        std::process::exit(1);
    }
    let fonts = SabiFonts::new(Kpse::default());
    let exec = PageExecutor::new(&dvi, &fonts, paper);
    let (list, report) = match exec.run_page(page - 1) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let (w, h) = (
        (paper.width * dpi / 72.0).ceil() as usize,
        (paper.height * dpi / 72.0).ceil() as usize,
    );
    let mut canvas = Canvas::filled(w, h, [1.0, 1.0, 1.0, 1.0]);
    let raster = render(&list, &mut canvas, &page_to_device(paper.height, dpi));
    let bytes = png::encode_rgba8(w as u32, h as u32, &canvas.to_rgba8());
    if let Err(e) = std::fs::write(&out, bytes) {
        eprintln!("{out}: {e}");
        std::process::exit(1);
    }
    eprintln!(
        "{out}: {w}x{h}, chars {}, rules {}, specials {}, missing widths {}, missing glyphs {}, unsupported {}",
        report.chars,
        report.rules,
        report.specials,
        report.missing_width,
        report.missing_glyph,
        report.unsupported.len() + raster.skipped.len()
    );
    for u in report.unsupported.iter().chain(raster.skipped.iter()) {
        eprintln!("  unsupported: {u}");
    }
}
