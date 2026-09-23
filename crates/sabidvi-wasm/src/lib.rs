//! ブラウザ向けの C ABI。SabiTeX の wasm と同じ流儀（wasm-bindgen を使わず、線形メモリの受け渡しだけ）。
//!
//! 使い方（JS 側）:
//!
//! 1. `sabidvi_alloc` で確保した領域に名前と内容を書き、`sabidvi_add_file` でフォント束に加える
//!    （`cmr10.tfm`、`cmr10.pfb`、`8r.enc`、`pdftex.map` など。SabiDVI が探す名前そのまま）
//! 2. `sabidvi_render(dvi, len, page, scale, margin)` を呼ぶ。戻り値 0 なら `sabidvi_pixels_ptr/len` に RGBA8、
//!    `sabidvi_meta_ptr/len` に JSON（画素の大きさ、インクの範囲、基線、未対応の数）
//! 3. 戻り値 1 なら足りないファイルがあり、字形の欠けた画像になっている。JSON の `missing` を取り寄せて 1 に戻る
//!    （SabiTeX の missing-file 方式）。取り寄せられないものがあれば、その画像をそのまま使ってよい
//! 4. 戻り値 2 は誤り。JSON の `error` に理由
//!
//! 座標: `scale` はページ空間の 1 bp あたりの画素数。`margin` はインクの範囲に足す余白（bp）。
//! 出力は透明な背景に黒で描いた RGBA8（プリマルチプライしない）。基線などの位置は bp で返すので、
//! 利用側は `data-page` の寸法と合わせて置ける。

use sabidvi_fonts::{MemoryLocator, SabiFonts};
use sabidvi_format::Dvi;
use sabidvi_page::{PageExecutor, Paper};
use sabirender_display::Matrix;
use sabirender_raster::{page_to_device, render, Canvas};
use sabirender_svg::bounds;
use std::cell::RefCell;

thread_local! {
    static FILES: RefCell<MemoryLocator> = RefCell::new(MemoryLocator::default());
    static PIXELS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static META: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// 線形メモリの確保。JS はここに書いてから他の関数に渡す
#[no_mangle]
pub extern "C" fn sabidvi_alloc(len: usize) -> *mut u8 {
    let mut v: Vec<u8> = Vec::with_capacity(len.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// `sabidvi_alloc` で確保した領域を返す
///
/// # Safety
/// `ptr` と `len` は `sabidvi_alloc(len)` の返り値と同じ組でなければならない
#[no_mangle]
pub unsafe extern "C" fn sabidvi_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(Vec::from_raw_parts(ptr, 0, len.max(1)));
    }
}

/// フォント束にファイルを加える（内容は複製する）
///
/// # Safety
/// 2 つの (ptr, len) は有効な領域を指していなければならない
#[no_mangle]
pub unsafe extern "C" fn sabidvi_add_file(
    name_ptr: *const u8,
    name_len: usize,
    data_ptr: *const u8,
    data_len: usize,
) -> i32 {
    let name = std::slice::from_raw_parts(name_ptr, name_len);
    let data = std::slice::from_raw_parts(data_ptr, data_len);
    let Ok(name) = std::str::from_utf8(name) else {
        return 2;
    };
    FILES.with(|f| f.borrow_mut().add(name, data.to_vec()));
    0
}

#[no_mangle]
pub extern "C" fn sabidvi_clear_files() {
    FILES.with(|f| *f.borrow_mut() = MemoryLocator::default());
}

#[no_mangle]
pub extern "C" fn sabidvi_file_count() -> usize {
    FILES.with(|f| f.borrow().len())
}

/// DVI のページ数。不正なら -1
///
/// # Safety
/// (ptr, len) は有効な領域
#[no_mangle]
pub unsafe extern "C" fn sabidvi_page_count(dvi_ptr: *const u8, dvi_len: usize) -> i32 {
    let data = std::slice::from_raw_parts(dvi_ptr, dvi_len);
    match Dvi::parse(data) {
        Ok(d) => d.pages.len() as i32,
        Err(_) => -1,
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn set_meta(json: String) {
    META.with(|m| *m.borrow_mut() = json.into_bytes());
}

fn fail(msg: &str) -> i32 {
    PIXELS.with(|p| p.borrow_mut().clear());
    set_meta(format!("{{\"error\":\"{}\"}}", json_escape(msg)));
    2
}

/// 1 ページを描く。戻り値: 0 = 成功、1 = 足りないファイルあり（meta の `missing`）、2 = 誤り（meta の `error`）
///
/// # Safety
/// (ptr, len) は有効な領域
#[no_mangle]
pub unsafe extern "C" fn sabidvi_render(
    dvi_ptr: *const u8,
    dvi_len: usize,
    page: u32,
    scale: f64,
    margin: f64,
) -> i32 {
    let data = std::slice::from_raw_parts(dvi_ptr, dvi_len);
    let dvi = match Dvi::parse(data) {
        Ok(d) => d,
        Err(e) => return fail(&e.to_string()),
    };
    if page == 0 || page as usize > dvi.pages.len() {
        return fail(&format!(
            "page {page} out of range (1..={})",
            dvi.pages.len()
        ));
    }
    if !(scale.is_finite() && scale > 0.0 && scale < 1000.0) {
        return fail("scale out of range");
    }
    // フォント束は毎回取り直す（追加されたファイルを反映するため）。束は複製せず借りたまま使う
    let result = FILES.with(|f| {
        let files = f.borrow();
        let fonts = SabiFonts::new(&*files);
        let exec = PageExecutor::new(&dvi, &fonts, Paper::A4);
        let (list, report) = match exec.run_page(page as usize - 1) {
            Ok(r) => r,
            Err(e) => return Err(e.to_string()),
        };
        // 足りないフォントがあっても描けるところまで描いて返す（利用側は `missing` を取り寄せてもう一度呼ぶ）
        let missing = fonts.missing();
        let incomplete =
            (report.missing_width > 0 || report.missing_glyph > 0) && !missing.is_empty();
        let paper = report.paper;
        let b = bounds(&list);
        let (bx, by, pw, ph) = match b {
            Some(b) => (
                b.xmin - margin,
                b.ymin - margin,
                b.width() + 2.0 * margin,
                b.height() + 2.0 * margin,
            ),
            None => (0.0, 0.0, 1.0, 1.0),
        };
        let cw = (pw * scale).ceil().max(1.0) as usize;
        let ch = (ph * scale).ceil().max(1.0) as usize;
        if cw * ch > 64 * 1024 * 1024 {
            return Err("image too large".into());
        }
        let mut canvas = Canvas::new(cw, ch);
        let m = Matrix::translate(-bx, -by).then(&page_to_device(ph, scale * 72.0));
        let raster = render(&list, &mut canvas, &m);
        let baseline = report
            .marks
            .iter()
            .find(|(n, _, _)| n == "baseline")
            .map(|(_, _, y)| *y)
            .unwrap_or(paper.height - 72.0);
        let (xmin, ymin, xmax, ymax) = match b {
            Some(b) => (b.xmin, b.ymin, b.xmax, b.ymax),
            None => (0.0, 0.0, 0.0, 0.0),
        };
        let unsupported: Vec<String> = report
            .unsupported
            .iter()
            .chain(raster.skipped.iter())
            .map(|s| format!("\"{}\"", json_escape(s)))
            .collect();
        let meta = format!(
            "{{\"width\":{cw},\"height\":{ch},\"scale\":{scale},\"margin\":{margin},\"xmin\":{xmin:.4},\"ymin\":{ymin:.4},\"xmax\":{xmax:.4},\"ymax\":{ymax:.4},\"baseline\":{baseline:.4},\"originX\":{bx:.4},\"originY\":{by:.4},\"chars\":{},\"missingGlyphs\":{},\"unsupported\":[{}],\"missing\":[{}]}}",
            report.chars,
            report.missing_glyph,
            unsupported.join(","),
            missing
                .iter()
                .map(|s| format!("\"{}\"", json_escape(s)))
                .collect::<Vec<_>>()
                .join(",")
        );
        Ok((canvas.to_rgba8(), meta, incomplete))
    });
    match result {
        Err(e) => fail(&e),
        Ok((pixels, meta, incomplete)) => {
            PIXELS.with(|p| *p.borrow_mut() = pixels);
            set_meta(meta);
            if incomplete {
                1
            } else {
                0
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn sabidvi_pixels_ptr() -> *const u8 {
    PIXELS.with(|p| p.borrow().as_ptr())
}

#[no_mangle]
pub extern "C" fn sabidvi_pixels_len() -> usize {
    PIXELS.with(|p| p.borrow().len())
}

#[no_mangle]
pub extern "C" fn sabidvi_meta_ptr() -> *const u8 {
    META.with(|m| m.borrow().as_ptr())
}

#[no_mangle]
pub extern "C" fn sabidvi_meta_len() -> usize {
    META.with(|m| m.borrow().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn special(ops: &mut Vec<u8>, s: &str) {
        ops.extend_from_slice(&[239, s.len() as u8]);
        ops.extend_from_slice(s.as_bytes());
    }

    /// 罫線 1 本と基線の目印だけの 1 ページ
    fn dvi_bytes() -> Vec<u8> {
        let mut ops = Vec::new();
        special(&mut ops, "sabidvi:mark baseline");
        ops.push(137);
        ops.extend_from_slice(&655360i32.to_be_bytes()); // 高さ 10pt
        ops.extend_from_slice(&(655360i32 * 2).to_be_bytes()); // 幅 20pt
        let mut d = vec![247, 2];
        for n in [25400000u32, 473628672, 1000] {
            d.extend_from_slice(&n.to_be_bytes());
        }
        d.push(0);
        let bop = d.len() as i32;
        d.push(139);
        d.extend_from_slice(&[0; 40]);
        d.extend_from_slice(&(-1i32).to_be_bytes());
        d.extend_from_slice(&ops);
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

    #[test]
    fn renders_a_rule_through_the_c_abi() {
        let dvi = dvi_bytes();
        let n = unsafe { sabidvi_page_count(dvi.as_ptr(), dvi.len()) };
        assert_eq!(n, 1);
        // 1 bp = 4 画素、余白 1 bp: 20pt × 10pt の罫線は (19.93 + 2) × 4 ≈ 88 画素 × (9.96 + 2) × 4 ≈ 48 画素
        let status = unsafe { sabidvi_render(dvi.as_ptr(), dvi.len(), 1, 4.0, 1.0) };
        let meta = String::from_utf8(META.with(|m| m.borrow().clone())).unwrap();
        assert_eq!(status, 0, "{meta}");
        assert!(meta.contains("\"width\":88,\"height\":48"), "{meta}");
        assert!(meta.contains("\"baseline\":769.8900"), "{meta}");
        let px = PIXELS.with(|p| p.borrow().clone());
        assert_eq!(px.len(), 88 * 48 * 4);
        // 中央は不透明の黒
        let i = (24 * 88 + 44) * 4;
        assert_eq!(&px[i..i + 4], &[0, 0, 0, 255]);
        // 余白は透明
        assert_eq!(px[3], 0);
    }

    #[test]
    fn missing_fonts_are_reported_for_the_second_pass() {
        // フォント定義付きの文字を置く DVI: cmr10 の 'A'
        let mut dvi = dvi_bytes();
        // page_ops の直前に fnt_def1 と fnt_num_0、set_char を挿入するのは面倒なので、postamble だけで判定する
        let _ = &mut dvi;
        sabidvi_clear_files();
        assert_eq!(sabidvi_file_count(), 0);
        let name = b"x.tfm";
        let data = [1u8, 2, 3];
        let r = unsafe { sabidvi_add_file(name.as_ptr(), name.len(), data.as_ptr(), data.len()) };
        assert_eq!(r, 0);
        assert_eq!(sabidvi_file_count(), 1);
        sabidvi_clear_files();
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        let junk = [1u8, 2, 3];
        let status = unsafe { sabidvi_render(junk.as_ptr(), junk.len(), 1, 1.0, 0.0) };
        assert_eq!(status, 2);
        let meta = String::from_utf8(META.with(|m| m.borrow().clone())).unwrap();
        assert!(meta.starts_with("{\"error\":"));
    }
}
