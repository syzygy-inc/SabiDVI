//! 幾何の同一性の検証: 同じ DVI を dvipdfmx で PDF にし、その内容ストリームを SabiRender の評価器で描画命令列に読み直して、
//! SabiDVI が出した描画命令列と比べる。ラスタライズを介さない。
//! 各テストは契約 case（`specification/cases.md`）。e-TeX・PGF・dvipdfmx が無ければ BLOCKED、起動したのに出力が無ければ FAIL。
//! 両側を同じ評価器で読むため評価器の共通の誤りは検出できない（手計算の期待値は `synthetic.rs`）。

use sabidvi_format::{Dvi, FontDef};
use sabidvi_page::{FontSource, PageExecutor, Paper};
use sabidvi_qa::{kpsewhich, run_tool, Case};
use sabirender_content::{Evaluator, NoResources};
use sabirender_display::{DisplayList, Item, Matrix, Path, Segment};

struct KpseTfm {
    cache:
        std::cell::RefCell<std::collections::HashMap<String, Option<sabiface_metrics::tfm::Tfm>>>,
}

impl KpseTfm {
    fn with<T>(&self, name: &str, f: impl FnOnce(&sabiface_metrics::tfm::Tfm) -> T) -> Option<T> {
        let mut cache = self.cache.borrow_mut();
        let entry = cache.entry(name.to_string()).or_insert_with(|| {
            let path = kpsewhich(&format!("{name}.tfm"))?;
            sabiface_metrics::tfm::Tfm::parse(&std::fs::read(path).ok()?).ok()
        });
        entry.as_ref().map(f)
    }
}

impl FontSource for KpseTfm {
    fn char_width(&self, font: &FontDef, code: u32) -> Option<f64> {
        self.with(&font.name, |tfm| {
            tfm.char_info(code as u16).map(|c| c.width.to_f64())
        })?
    }
    fn glyph(&self, _: &FontDef, _: u32) -> Option<(Path, Matrix)> {
        None
    }
}

const SOURCE: &str = r#"\def\pgfsysdriver{pgfsys-dvipdfmx.def}
\input tikz
\tikzpicture
\draw[line width=2pt] (0,0) -- (1,1);
\fill[red] (2,0) rectangle (3,1);
\draw[blue, dashed, rounded corners] (0,2) -- (1,2) -- (1,3);
\draw[fill=green!50, opacity=0.5, line width=0.5pt] (2,2) circle (0.4);
\clip (0,4) rectangle (2,5);
\fill (1,4) circle (2);
\endtikzpicture
\bye
"#;

/// etex で DVI を、dvipdfmx で非圧縮の PDF を作る
fn build(case: &Case) -> Option<(Vec<u8>, Vec<u8>)> {
    let dir = std::env::temp_dir().join(format!("sabidvi-pgf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("p.tex"), SOURCE).unwrap();
    if run_tool(
        case,
        "etex",
        &["-interaction=batchmode", "p.tex"],
        Some(&dir),
    )
    .is_none()
    {
        case.blocked("etex not found");
        return None;
    }
    let Ok(dvi) = std::fs::read(dir.join("p.dvi")) else {
        case.tool_failed("etex produced no DVI (is PGF installed?)");
    };
    if run_tool(
        case,
        "dvipdfmx",
        &["-z0", "-o", "p.pdf", "p.dvi"],
        Some(&dir),
    )
    .is_none()
    {
        case.blocked("dvipdfmx not found");
        return None;
    }
    let Ok(pdf) = std::fs::read(dir.join("p.pdf")) else {
        case.tool_failed("dvipdfmx produced no PDF");
    };
    let _ = std::fs::remove_dir_all(&dir);
    Some((dvi, pdf))
}

/// 非圧縮 PDF から最初のページの内容ストリームと MediaBox を取り出す。
/// 先頭に二進数のコメント行があるので、文字列にせずバイト列のまま探す
fn page_content(pdf: &[u8]) -> (Vec<u8>, Paper) {
    let find = |s: &str, from: usize| {
        pdf[from..]
            .windows(s.len())
            .position(|w| w == s.as_bytes())
            .map(|i| i + from)
    };
    let slice = |a: usize, b: usize| String::from_utf8_lossy(&pdf[a..b]).into_owned();
    let media = find("/MediaBox[", 0).expect("MediaBox");
    let end = find("]", media).unwrap();
    let nums: Vec<f64> = slice(media + 10, end)
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    let paper = Paper {
        width: nums[2] - nums[0],
        height: nums[3] - nums[1],
    };
    let contents = find("/Contents[", 0).expect("Contents");
    let cend = find(" 0 R", contents).unwrap();
    let obj_num = slice(contents + 10, cend).trim().to_string();
    let obj = find(&format!("\n{obj_num} 0 obj"), 0).expect("content object");
    let s = find("stream", obj).unwrap() + "stream".len();
    let s = if pdf[s] == b'\r' { s + 2 } else { s + 1 };
    let e = find("endstream", s).unwrap();
    (pdf[s..e].to_vec(), paper)
}

#[derive(Debug, PartialEq)]
struct Shape {
    kind: &'static str,
    points: Vec<(f64, f64)>,
    color: (f64, f64, f64),
    width: f64,
    dash: Vec<f64>,
    alpha: f64,
}

fn shapes(list: &DisplayList) -> Vec<Shape> {
    let mut v = Vec::new();
    for item in &list.items {
        let (kind, path, ctm, color, width, dash, alpha) = match item {
            Item::Fill {
                path,
                ctm,
                color,
                alpha,
                ..
            } => ("fill", path, ctm, color, 0.0, Vec::new(), *alpha),
            Item::Stroke {
                path,
                ctm,
                style,
                color,
                alpha,
            } => (
                "stroke",
                path,
                ctm,
                color,
                style.width * ctm.mean_scale(),
                style.dash.clone(),
                *alpha,
            ),
            Item::ClipPush { path, ctm, .. } => (
                "clip",
                path,
                ctm,
                &sabirender_display::Color::BLACK,
                0.0,
                Vec::new(),
                1.0,
            ),
            _ => continue,
        };
        let p = path.transform(ctm);
        let mut points = Vec::new();
        for s in &p.segments {
            match *s {
                Segment::MoveTo(x, y) | Segment::LineTo(x, y) => points.push((x, y)),
                Segment::CurveTo(a, b, c, d, e, f) => {
                    points.push((a, b));
                    points.push((c, d));
                    points.push((e, f));
                }
                Segment::Close => {}
            }
        }
        v.push(Shape {
            kind,
            points,
            color: color.to_rgb(),
            width,
            dash,
            alpha,
        });
    }
    v
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// DVI-PGF-DVIPDFMX
#[test]
fn pgf_picture_matches_dvipdfmx_geometry() {
    let case = Case::required("DVI-PGF-DVIPDFMX", &["C-DVI", "C-DRAW"]);
    let Some((dvi_bytes, pdf_bytes)) = build(&case) else {
        return;
    };
    let (content, paper) = page_content(&pdf_bytes);

    // dvipdfmx の PDF を幾何として読み直す（SabiPDF の入口と同じ評価器）。
    // 不透明度は資源辞書（/pgf@ca0.5 など）にあり、この読み直しでは解決しないので比べない
    let mut from_pdf = DisplayList::default();
    let mut ev = Evaluator::new(Matrix::IDENTITY, &NoResources);
    ev.run(&content, &mut from_pdf);
    ev.finish(&mut from_pdf);

    // SabiDVI で DVI を描画命令列にする
    let dvi = Dvi::parse(&dvi_bytes).unwrap();
    let fonts = KpseTfm {
        cache: Default::default(),
    };
    let exec = PageExecutor::new(&dvi, &fonts, paper);
    let (from_dvi, report) = exec.run_page(0).unwrap();
    assert_eq!(
        report.missing_width, 0,
        "TFM widths should resolve via kpsewhich"
    );
    assert!(report.unsupported.is_empty(), "{:?}", report.unsupported);

    let a = shapes(&from_dvi);
    let b = shapes(&from_pdf);
    assert!(!a.is_empty());
    assert_eq!(
        a.len(),
        b.len(),
        "shape count differs:\n{a:#?}\n----\n{b:#?}"
    );
    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
        assert_eq!(x.kind, y.kind, "shape {i}");
        assert_eq!(
            x.points.len(),
            y.points.len(),
            "shape {i} point count: {x:?} vs {y:?}"
        );
        for (p, q) in x.points.iter().zip(&y.points) {
            // dvipdfmx は座標を小数 3〜5 桁で書く
            assert!(
                close(p.0, q.0, 2e-3) && close(p.1, q.1, 2e-3),
                "shape {i}: {p:?} vs {q:?}"
            );
            case.compared();
        }
        for k in 0..3 {
            let (c1, c2) = (
                [x.color.0, x.color.1, x.color.2][k],
                [y.color.0, y.color.1, y.color.2][k],
            );
            assert!(
                close(c1, c2, 1e-6),
                "shape {i} color {:?} vs {:?}",
                x.color,
                y.color
            );
        }
        assert!(
            close(x.width, y.width, 1e-4),
            "shape {i} width {} vs {}",
            x.width,
            y.width
        );
        assert_eq!(x.dash.len(), y.dash.len(), "shape {i} dash");
        case.compared_n(3);
    }
    // 期待する図形: 線、矩形の塗り、破線、円の塗りと線、クリップ、クリップされた塗り
    let kinds: Vec<&str> = a.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        ["stroke", "fill", "stroke", "fill", "stroke", "clip", "fill"]
    );
    let stroke = &a[0];
    // 最初の線は (0,0)–(1cm,1cm)。1cm = 28.3465bp
    let (p, q) = (stroke.points[0], stroke.points[1]);
    assert!(
        close(q.0 - p.0, 28.3465, 1e-2) && close(q.1 - p.1, 28.3465, 1e-2),
        "{stroke:?}"
    );
    assert!(close(stroke.width, 2.0 * 72.0 / 72.27, 1e-4));
    // 不透明度 0.5 は pdf:put の拡張図形状態から解決される
    assert!(
        close(a[3].alpha, 0.5, 1e-9) && close(a[4].alpha, 0.5, 1e-9),
        "{:?} {:?}",
        a[3].alpha,
        a[4].alpha
    );
    case.compared_n(4);
    case.done();
}

/// DVI-TFM-POSITION: 文字と規則の位置を TFM の幅と手計算で確かめる
#[test]
fn text_and_rules_are_positioned_from_tfm_widths() {
    let case = Case::required("DVI-TFM-POSITION", &["C-DVI", "C-FONT"]);
    let dir = std::env::temp_dir().join(format!("sabidvi-rule-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("r.tex"),
        "\\font\\a=cmr10 \\a AB\\vrule width 10pt height 5pt depth 1pt\\bye\n",
    )
    .unwrap();
    if run_tool(
        &case,
        "tex",
        &["-interaction=batchmode", "r.tex"],
        Some(&dir),
    )
    .is_none()
    {
        return case.blocked("tex not found");
    }
    let Ok(bytes) = std::fs::read(dir.join("r.dvi")) else {
        case.tool_failed("tex produced no DVI");
    };
    let _ = std::fs::remove_dir_all(&dir);
    let dvi = Dvi::parse(&bytes).unwrap();
    let fonts = KpseTfm {
        cache: Default::default(),
    };
    let exec = PageExecutor::new(&dvi, &fonts, Paper::A4);
    let (list, report) = exec.run_page(0).unwrap();
    assert_eq!(report.chars, 3, "A, B and the page number");
    assert_eq!(report.missing_width, 0);
    let rule = list
        .items
        .iter()
        .find_map(|i| {
            if let Item::Fill { path, .. } = i {
                Some(path)
            } else {
                None
            }
        })
        .expect("rule");
    // 規則は 10pt × 6pt（高さ 5pt + 深さ 1pt）
    let (x0, y0) = match rule.segments[0] {
        Segment::MoveTo(x, y) => (x, y),
        _ => unreachable!(),
    };
    let (x1, y1) = match rule.segments[2] {
        Segment::LineTo(x, y) => (x, y),
        _ => unreachable!(),
    };
    assert!(close(x1 - x0, 10.0 * 72.0 / 72.27, 1e-6));
    assert!(close(y1 - y0, 6.0 * 72.0 / 72.27, 1e-6));
    // plain TeX の \parindent = 20pt の後に A の幅 (0.75 em) + B の幅 (0.708334 em)。ページ左端の 72bp を足す
    let expected_x = 72.0 + (20.0 + (0.75 + 0.708334) * 10.0) * 72.0 / 72.27;
    assert!(close(x0, expected_x, 1e-3), "{x0} vs {expected_x}");
    case.compared_n(5);
    case.done();
}
