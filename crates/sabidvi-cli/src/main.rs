//! `samatch bounds(&list).zip(crop) {idvi render file.dvi [-o out.png|out.svg] [--page N] [--dpi D] [--paper a4|letter] [--crop [margin]]`
//!
//! DVI / XDV の 1 ページを、出力名の拡張子に応じて PNG（参照ラスタライザ）か SVG（経路のまま）にする。
//! `--crop` はインクの範囲に切り詰める（数式の埋め込み用）。フォントは kpsewhich で探す。

use sabidvi_fonts::{Kpse, SabiFonts};
use sabidvi_format::Dvi;
use sabidvi_page::{PageExecutor, Paper};
use sabirender_raster::{page_to_device, png, render, Canvas};
use sabirender_svg::{bounds, to_svg, SvgOptions};

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|s| s.as_str()) != Some("render") || args.len() < 2 {
        eprintln!("usage: sabidvi render <file.dvi|file.xdv> [-o out.png|out.svg] [--page N] [--dpi D] [--paper a4|letter] [--crop [margin_bp]]");
        std::process::exit(2);
    }
    let input = &args[1];
    let mut out = "out.png".to_string();
    let mut page = 1usize;
    let mut dpi = 150.0;
    let mut paper = Paper::A4;
    let mut crop: Option<f64> = None;
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
            "--crop" => {
                // 省略可能な余白（bp）
                match args.get(i + 1).and_then(|v| v.parse::<f64>().ok()) {
                    Some(m) => {
                        crop = Some(m);
                        i += 2;
                    }
                    None => {
                        crop = Some(1.0);
                        i += 1;
                    }
                }
            }
            other => {
                eprintln!("unknown option {other}");
                std::process::exit(2);
            }
        }
    }
    let data = std::fs::read(input).unwrap_or_else(|e| fail(format!("{input}: {e}")));
    let dvi = Dvi::parse(&data).unwrap_or_else(|e| fail(e));
    if page == 0 || page > dvi.pages.len() {
        fail(format!(
            "page {page} out of range (1..={})",
            dvi.pages.len()
        ));
    }
    let fonts = SabiFonts::new(Kpse::default());
    let exec = PageExecutor::new(&dvi, &fonts, paper);
    let (list, report) = exec.run_page(page - 1).unwrap_or_else(|e| fail(e));

    let mut skipped: Vec<String> = Vec::new();
    let (w, h);
    if out.ends_with(".svg") {
        let opts = SvgOptions {
            margin: crop.unwrap_or(0.0),
            bounds: if crop.is_some() {
                None
            } else {
                Some(sabirender_svg::Bounds {
                    xmin: 0.0,
                    ymin: 0.0,
                    xmax: paper.width,
                    ymax: paper.height,
                })
            },
            ..Default::default()
        };
        let svg = to_svg(&list, &opts);
        std::fs::write(&out, svg).unwrap_or_else(|e| fail(format!("{out}: {e}")));
        let b = opts
            .bounds
            .or_else(|| bounds(&list))
            .unwrap_or(sabirender_svg::Bounds {
                xmin: 0.0,
                ymin: 0.0,
                xmax: 1.0,
                ymax: 1.0,
            });
        (w, h) = (
            b.width() + 2.0 * opts.margin,
            b.height() + 2.0 * opts.margin,
        );
        eprintln!("{out}: {:.3} x {:.3} bp", w, h);
    } else {
        let s = dpi / 72.0;
        // 切り詰めるときは、インクの範囲を画素に合わせて広げた矩形を装置空間の原点にする
        let (bx, by, pw, ph) = match bounds(&list).zip(crop) {
            Some((b, m)) => (
                (b.xmin - m).floor(),
                (b.ymin - m).floor(),
                (b.width() + 2.0 * m).ceil(),
                (b.height() + 2.0 * m).ceil(),
            ),
            None => (0.0, 0.0, paper.width, paper.height),
        };
        let (cw, ch) = ((pw * s).ceil() as usize, (ph * s).ceil() as usize);
        let mut canvas = Canvas::filled(cw, ch, [1.0, 1.0, 1.0, 1.0]);
        // ページ空間 → 装置: x' = (x - bx) s, y' = (by + ph - y) s
        let m =
            sabirender_raster::display::Matrix::translate(-bx, -by).then(&page_to_device(ph, dpi));
        let raster = render(&list, &mut canvas, &m);
        skipped = raster.skipped;
        let bytes = png::encode_rgba8(cw as u32, ch as u32, &canvas.to_rgba8());
        std::fs::write(&out, bytes).unwrap_or_else(|e| fail(format!("{out}: {e}")));
        (w, h) = (cw as f64, ch as f64);
        eprintln!("{out}: {cw}x{ch}");
    }
    let _ = (w, h);
    eprintln!(
        "chars {}, rules {}, specials {}, missing widths {}, missing glyphs {}, unsupported {}",
        report.chars,
        report.rules,
        report.specials,
        report.missing_width,
        report.missing_glyph,
        report.unsupported.len() + skipped.len()
    );
    for u in report.unsupported.iter().chain(skipped.iter()) {
        eprintln!("  unsupported: {u}");
    }
}
