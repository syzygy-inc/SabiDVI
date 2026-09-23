//! 手で組み立てた最小の DVI による、手計算の期待値との照合。TeX Live もフォントも要らない。
//! （レビュー報告 D1〜D4 の再現入力を回帰テストにしたもの）

use sabidvi_format::Dvi;
use sabidvi_page::{NoFonts, PageExecutor, PageReport, Paper};
use sabirender_display::{DisplayList, Item, Segment};

fn special(ops: &mut Vec<u8>, s: &str) {
    ops.extend_from_slice(&[239, s.len() as u8]);
    ops.extend_from_slice(s.as_bytes());
}

/// 1 ページの DVI。`prev` を指定すると bop の前ページポインタを差し替える（既定は -1）
fn dvi_bytes_with_prev(ops: &[u8], prev: Option<i32>) -> Vec<u8> {
    let mut d = vec![247, 2];
    for n in [25400000u32, 473628672, 1000] {
        d.extend_from_slice(&n.to_be_bytes());
    }
    d.push(0);
    let bop = d.len() as i32;
    d.push(139);
    d.extend_from_slice(&[0; 40]);
    d.extend_from_slice(&prev.unwrap_or(-1).to_be_bytes());
    d.extend_from_slice(ops);
    d.push(140);
    let post = d.len() as i32;
    d.push(248);
    d.extend_from_slice(&bop.to_be_bytes());
    for n in [25400000u32, 473628672, 1000, 10000000, 10000000] {
        d.extend_from_slice(&n.to_be_bytes());
    }
    d.extend_from_slice(&[0, 1, 0, 1, 249]);
    d.extend_from_slice(&post.to_be_bytes());
    d.extend_from_slice(&[2, 223, 223, 223, 223]);
    d
}

fn dvi_bytes(ops: &[u8]) -> Vec<u8> {
    dvi_bytes_with_prev(ops, None)
}

fn run(ops: &[u8]) -> (DisplayList, PageReport) {
    let bytes = dvi_bytes(ops);
    let dvi = Dvi::parse(&bytes).unwrap();
    PageExecutor::new(&dvi, &NoFonts, Paper::A4)
        .run_page(0)
        .unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn rect_of(item: &Item) -> (f64, f64, f64, f64) {
    let Item::Fill { path, ctm, .. } = item else {
        panic!("{item:?}")
    };
    let p = path.transform(ctm);
    let pts: Vec<(f64, f64)> = p
        .segments
        .iter()
        .filter_map(|s| match *s {
            Segment::MoveTo(x, y) | Segment::LineTo(x, y) => Some((x, y)),
            _ => None,
        })
        .collect();
    let xs = pts.iter().map(|p| p.0);
    let ys = pts.iter().map(|p| p.1);
    (
        xs.clone().fold(f64::MAX, f64::min),
        ys.clone().fold(f64::MAX, f64::min),
        xs.fold(f64::MIN, f64::max),
        ys.fold(f64::MIN, f64::max),
    )
}

/// D1: papersize は紙面と座標変換に効く
#[test]
fn papersize_special_changes_the_page_frame() {
    let mut ops = Vec::new();
    special(&mut ops, "papersize=200bp,300bp");
    special(&mut ops, "sabidvi:mark baseline");
    let (_, report) = run(&ops);
    assert_eq!(
        report.paper,
        Paper {
            width: 200.0,
            height: 300.0
        }
    );
    let (name, x, y) = &report.marks[0];
    assert_eq!(name, "baseline");
    assert!(close(*x, 72.0) && close(*y, 300.0 - 72.0), "{x} {y}");
    assert!(report.unsupported.is_empty());
    // pdf:pagesize でも同じ。special がページの後方にあってもページ全体に効く
    let mut ops = Vec::new();
    special(&mut ops, "sabidvi:mark first");
    special(&mut ops, "pdf:pagesize width 100bp height 50bp");
    let (_, report) = run(&ops);
    assert_eq!(
        report.paper,
        Paper {
            width: 100.0,
            height: 50.0
        }
    );
    assert!(close(report.marks[0].2, 50.0 - 72.0));
}

/// D2: btrans の変換は罫線にも効く
#[test]
fn transform_specials_apply_to_rules() {
    let mut ops = Vec::new();
    special(&mut ops, "pdf:btrans scale 2");
    ops.push(137); // put_rule 10pt × 10pt
    ops.extend_from_slice(&655360i32.to_be_bytes());
    ops.extend_from_slice(&655360i32.to_be_bytes());
    special(&mut ops, "pdf:etrans");
    let (list, report) = run(&ops);
    assert!(report.unsupported.is_empty());
    let (x0, y0, x1, y1) = rect_of(&list.items[0]);
    let pt = 72.0 / 72.27;
    // 現在位置 (72, 769.89) を中心に 2 倍: 罫線は 20pt 四方、左下は現在位置のまま
    assert!(close(x1 - x0, 20.0 * pt), "width {}", x1 - x0);
    assert!(close(y1 - y0, 20.0 * pt), "height {}", y1 - y0);
    assert!(close(x0, 72.0) && close(y0, 841.89 - 72.0), "{x0} {y0}");
    // 変換の外の罫線は元の大きさ
    let mut ops = Vec::new();
    ops.push(137);
    ops.extend_from_slice(&655360i32.to_be_bytes());
    ops.extend_from_slice(&655360i32.to_be_bytes());
    let (list, _) = run(&ops);
    let (x0, _, x1, _) = rect_of(&list.items[0]);
    assert!(close(x1 - x0, 10.0 * pt));
}

/// D2: bcontent の中の btrans。原点の平行移動と変換を二重に掛けない
#[test]
fn transform_inside_bcontent_keeps_the_origin() {
    let mut ops = Vec::new();
    ops.push(143); // right1 100 sp（原点をずらす）
    ops.push(100);
    special(&mut ops, "pdf:bcontent");
    special(&mut ops, "pdf:btrans scale 3");
    special(&mut ops, "sabidvi:mark here");
    special(&mut ops, "pdf:etrans");
    special(&mut ops, "pdf:econtent");
    let (_, report) = run(&ops);
    // 変換の中心は現在位置なので、現在位置の目印は動かない
    let (_, x, y) = report.marks[0];
    let bp = 72.0 / (72.27 * 65536.0);
    assert!(
        close(x, 72.0 + 100.0 * bp) && close(y, 841.89 - 72.0),
        "{x} {y}"
    );
}

/// D3: pop は組方向も戻す
#[test]
fn pop_restores_the_writing_direction() {
    let mut ops = vec![141, 255, 1, 142, 146]; // push; dirchg tate; pop; right4 65536
    ops.extend_from_slice(&65536i32.to_be_bytes());
    special(&mut ops, "sabidvi:mark after_pop");
    let (_, report) = run(&ops);
    let (_, x, y) = report.marks[0];
    // 横組に戻っているので right は x を増やす
    assert!(
        close(x, 72.0 + 72.0 / 72.27) && close(y, 841.89 - 72.0),
        "{x} {y}"
    );
    // 対照: pop しなければ縦組のまま（right は下向き）
    let mut ops = vec![141, 255, 1, 146];
    ops.extend_from_slice(&65536i32.to_be_bytes());
    special(&mut ops, "sabidvi:mark tate");
    let (_, report) = run(&ops);
    let (_, x, y) = report.marks[0];
    assert!(
        close(x, 72.0) && close(y, 841.89 - 72.0 - 72.0 / 72.27),
        "{x} {y}"
    );
}

/// D4: bop の前ページポインタが自分自身や後ろを指す DVI は誤りとして終わる
#[test]
fn self_referencing_bop_chain_is_an_error() {
    let bop = 15; // pre(1) + i[1] + num/den/mag(12) + k[1] = 15
    let bytes = dvi_bytes_with_prev(&[], Some(bop));
    assert_eq!(bytes[bop as usize], 139);
    assert!(Dvi::parse(&bytes).is_err());
    // 後ろを指す
    let bytes = dvi_bytes_with_prev(&[], Some(bop + 41));
    assert!(Dvi::parse(&bytes).is_err());
    // 正常
    assert!(Dvi::parse(&dvi_bytes(&[])).is_ok());
}
