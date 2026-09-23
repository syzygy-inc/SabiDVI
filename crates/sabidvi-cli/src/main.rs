//! `sabidvi render file.dvi [-o out.png|out.svg] [--page N|all] [--dpi D] [--paper a4|letter] [--crop [margin]] [--meta out.json]`
//!
//! DVI / XDV のページを、出力名の拡張子に応じて PNG（参照ラスタライザ）か SVG（経路のまま）にする。
//! `--page all` は全ページを `out-1.svg`、`out-2.svg`、… に書く（`--meta` も同様に `meta-1.json`、…）。
//! `--crop` はインクの範囲に切り詰め、`--meta` はインクの範囲と基線（`\special{sabidvi:mark baseline}`）を JSON に書く
//! （数式の埋め込み用）。フォントは TeX Live の ls-R と kpsewhich で探す。

use sabidvi_fonts::{Kpse, SabiFonts};
use sabidvi_format::Dvi;
use sabidvi_page::{PageExecutor, Paper};
use sabirender_raster::{page_to_device, png, render, Canvas};
use sabirender_svg::{bounds, to_svg, SvgOptions};

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

/// `out.svg` と 3 → `out-3.svg`
fn numbered(path: &str, n: usize) -> String {
    match path.rfind('.') {
        Some(i) if !path[i..].contains(['/', '\\']) => format!("{}-{n}{}", &path[..i], &path[i..]),
        _ => format!("{path}-{n}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|s| s.as_str()) != Some("render") || args.len() < 2 {
        eprintln!("usage: sabidvi render <file.dvi|file.xdv> [-o out.png|out.svg] [--page N|all] [--dpi D] [--paper a4|letter] [--crop [margin_bp]] [--meta out.json]");
        std::process::exit(2);
    }
    let input = &args[1];
    let mut out = "out.png".to_string();
    let mut page: Option<usize> = Some(1);
    let mut dpi = 150.0;
    let mut paper = Paper::A4;
    let mut crop: Option<f64> = None;
    let mut meta: Option<String> = None;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                out = args.get(i + 1).cloned().unwrap_or(out);
                i += 2;
            }
            "--page" => {
                page = match args.get(i + 1).map(|s| s.as_str()) {
                    Some("all") => None,
                    Some(v) => Some(v.parse().unwrap_or(1)),
                    None => Some(1),
                };
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
            "--meta" => {
                meta = args.get(i + 1).cloned();
                i += 2;
            }
            "--crop" => match args.get(i + 1).and_then(|v| v.parse::<f64>().ok()) {
                Some(m) => {
                    crop = Some(m);
                    i += 2;
                }
                None => {
                    crop = Some(1.0);
                    i += 1;
                }
            },
            other => {
                eprintln!("unknown option {other}");
                std::process::exit(2);
            }
        }
    }
    let data = std::fs::read(input).unwrap_or_else(|e| fail(format!("{input}: {e}")));
    let dvi = Dvi::parse(&data).unwrap_or_else(|e| fail(e));
    let pages: Vec<usize> = match page {
        Some(p) if p == 0 || p > dvi.pages.len() => {
            fail(format!("page {p} out of range (1..={})", dvi.pages.len()))
        }
        Some(p) => vec![p],
        None => (1..=dvi.pages.len()).collect(),
    };
    let fonts = SabiFonts::new(Kpse::default());
    let exec = PageExecutor::new(&dvi, &fonts, paper);
    let multi = page.is_none();
    let mut failures = 0usize;
    for p in pages {
        let out_path = if multi {
            numbered(&out, p)
        } else {
            out.clone()
        };
        let meta_path = meta
            .as_ref()
            .map(|m| if multi { numbered(m, p) } else { m.clone() });
        let (list, report) = exec.run_page(p - 1).unwrap_or_else(|e| fail(e));
        // 紙面は special で変わり得るので、ページごとの実効値を使う
        let paper = report.paper;
        let mut skipped: Vec<String> = Vec::new();
        if out_path.ends_with(".svg") {
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
            std::fs::write(&out_path, to_svg(&list, &opts))
                .unwrap_or_else(|e| fail(format!("{out_path}: {e}")));
            eprintln!("{out_path}");
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
            let m = sabirender_raster::display::Matrix::translate(-bx, -by)
                .then(&page_to_device(ph, dpi));
            let raster = render(&list, &mut canvas, &m);
            skipped = raster.skipped;
            let bytes = png::encode_rgba8(cw as u32, ch as u32, &canvas.to_rgba8());
            std::fs::write(&out_path, bytes).unwrap_or_else(|e| fail(format!("{out_path}: {e}")));
            eprintln!("{out_path}: {cw}x{ch}");
        }
        if let Some(meta_path) = meta_path {
            // 基線は `\special{sabidvi:mark baseline}` があればその y、無ければ DVI の原点の y（\shipout した箱の上端）
            let baseline = report
                .marks
                .iter()
                .find(|(n, _, _)| n == "baseline")
                .map(|(_, _, y)| *y)
                .unwrap_or(paper.height - 72.0);
            let json = match bounds(&list) {
                Some(b) => format!(
                    "{{\"page\":{p},\"xmin\":{:.4},\"ymin\":{:.4},\"xmax\":{:.4},\"ymax\":{:.4},\"baseline\":{:.4},\"depth\":{:.4},\"height\":{:.4},\"unsupported\":{}}}",
                    b.xmin,
                    b.ymin,
                    b.xmax,
                    b.ymax,
                    baseline,
                    baseline - b.ymin,
                    b.ymax - baseline,
                    report.unsupported.len() + skipped.len()
                ),
                None => "null".to_string(),
            };
            std::fs::write(&meta_path, json).unwrap_or_else(|e| fail(format!("{meta_path}: {e}")));
        }
        eprintln!(
            "page {p}: chars {}, rules {}, specials {}, missing widths {}, missing glyphs {}, unsupported {}",
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
        if report.missing_glyph > 0 || !report.unsupported.is_empty() || !skipped.is_empty() {
            failures += 1;
        }
    }
    if failures > 0 {
        std::process::exit(3);
    }
}
