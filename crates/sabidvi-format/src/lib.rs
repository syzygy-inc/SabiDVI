//! DVI ファイルの構文（tex.web §583〜§600）と、その拡張:
//!
//! - pTeX / upTeX（`ptex-base.ch`）: `dirchg` = 255 `d[1]`（0 = 横組、1 = 縦組、それ以外は dtou）。id は 2、縦組を含むと後付けの id が 3
//! - XeTeX の XDV（`xetex.web` §"Commands 250--255"、id = 7）: `pic_file` = 251、`define_native_font` = 252、`set_glyphs` = 253、
//!   `set_text_and_glyphs` = 254
//!
//! ここでは構文だけを扱い、位置の計算（h, v, w, x, y, z のスタック）は `sabidvi-page` が行う。

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DviError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for DviError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DVI error at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for DviError {}

fn err<T>(offset: usize, message: impl Into<String>) -> Result<T, DviError> {
    Err(DviError {
        offset,
        message: message.into(),
    })
}

/// DVI の種類（preamble の id）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// id = 2。pTeX の横組のみの出力もここ
    Dvi,
    /// id = 7（XeTeX 0.99996 以降）
    Xdv,
}

/// TFM を使うフォントの定義（`fnt_def`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontDef {
    pub number: u32,
    pub checksum: u32,
    /// 拡大後のサイズ（sp）
    pub scaled_size: i32,
    /// デザインサイズ（sp）
    pub design_size: i32,
    pub area: String,
    pub name: String,
}

/// XDV のネイティブフォント（`define_native_font`）
#[derive(Debug, Clone, PartialEq)]
pub struct NativeFontDef {
    pub number: u32,
    /// ポイントサイズ（sp）
    pub size: i32,
    pub flags: u16,
    /// フォントファイルのパス（XeTeX は `[path]` 形式や名前をそのまま書く）
    pub name: String,
    /// TTC などのフォントの添字
    pub index: u32,
    pub rgba: Option<[u8; 4]>,
    pub extend: Option<f64>,
    pub slant: Option<f64>,
    pub embolden: Option<f64>,
}

impl NativeFontDef {
    pub const FLAG_VERTICAL: u16 = 0x0100;
    pub const FLAG_COLORED: u16 = 0x0200;
    pub const FLAG_EXTEND: u16 = 0x1000;
    pub const FLAG_SLANT: u16 = 0x2000;
    pub const FLAG_EMBOLDEN: u16 = 0x4000;

    pub fn is_vertical(&self) -> bool {
        self.flags & Self::FLAG_VERTICAL != 0
    }
}

/// XDV の `set_glyphs` / `set_text_and_glyphs`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glyphs {
    /// 全体の送り（sp）
    pub width: i32,
    /// 字形ごとの (x, y) オフセット（sp）と字形 ID
    pub positions: Vec<(i32, i32)>,
    pub ids: Vec<u16>,
    /// `set_text_and_glyphs` の UTF-16 テキスト（検索・コピー用。描画には使わない）
    pub text: Vec<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Yoko,
    Tate,
    /// 下から上（pTeX の dtou）
    Dtou,
}

/// 1 命令。文字と規則は set（送る）と put（送らない）を区別する
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    SetChar(u32),
    PutChar(u32),
    /// 高さ a、幅 b（sp）
    SetRule {
        height: i32,
        width: i32,
    },
    PutRule {
        height: i32,
        width: i32,
    },
    Nop,
    Bop {
        counts: [i32; 10],
        prev: i32,
    },
    Eop,
    Push,
    Pop,
    Right(i32),
    W0,
    W(i32),
    X0,
    X(i32),
    Down(i32),
    Y0,
    Y(i32),
    Z0,
    Z(i32),
    Font(u32),
    /// `xxx1`〜`xxx4`。中身はそのまま（special の解釈は `sabidvi-special`）
    Special(Vec<u8>),
    FontDef(FontDef),
    Pre {
        id: u8,
        num: u32,
        den: u32,
        mag: u32,
        comment: Vec<u8>,
    },
    Post,
    PostPost,
    // --- pTeX ---
    Dir(Direction),
    // --- XDV ---
    /// `pic_file`: 変換行列（6 つの 32 ビット固定小数、2^16 単位）、ページ番号、パス
    PicFile {
        flags: u8,
        matrix: [i32; 6],
        page: u16,
        path: Vec<u8>,
    },
    NativeFontDef(NativeFontDef),
    SetGlyphs(Glyphs),
}

pub struct Reader<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn need(&self, n: usize) -> Result<(), DviError> {
        if self.pos + n > self.data.len() {
            return err(self.pos, format!("need {n} bytes"));
        }
        Ok(())
    }

    pub fn u8(&mut self) -> Result<u8, DviError> {
        self.need(1)?;
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    pub fn u16(&mut self) -> Result<u16, DviError> {
        self.need(2)?;
        let v = u16::from_be_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    pub fn u24(&mut self) -> Result<u32, DviError> {
        self.need(3)?;
        let d = &self.data[self.pos..self.pos + 3];
        self.pos += 3;
        Ok(((d[0] as u32) << 16) | ((d[1] as u32) << 8) | d[2] as u32)
    }

    pub fn u32(&mut self) -> Result<u32, DviError> {
        self.need(4)?;
        let d = &self.data[self.pos..self.pos + 4];
        self.pos += 4;
        Ok(u32::from_be_bytes([d[0], d[1], d[2], d[3]]))
    }

    pub fn i32(&mut self) -> Result<i32, DviError> {
        self.u32().map(|v| v as i32)
    }

    /// 符号付きの 1〜4 バイト
    pub fn signed(&mut self, n: u8) -> Result<i32, DviError> {
        Ok(match n {
            1 => self.u8()? as i8 as i32,
            2 => self.u16()? as i16 as i32,
            3 => {
                let v = self.u24()?;
                if v & 0x80_0000 != 0 {
                    (v | 0xFF00_0000) as i32
                } else {
                    v as i32
                }
            }
            _ => self.i32()?,
        })
    }

    /// 符号なしの 1〜4 バイト
    pub fn unsigned(&mut self, n: u8) -> Result<u32, DviError> {
        Ok(match n {
            1 => self.u8()? as u32,
            2 => self.u16()? as u32,
            3 => self.u24()?,
            _ => self.u32()?,
        })
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], DviError> {
        self.need(n)?;
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// 1 命令を読む。`kind` により 250〜255 の解釈を変える
    pub fn op(&mut self, kind: Kind) -> Result<Op, DviError> {
        let at = self.pos;
        let c = self.u8()?;
        Ok(match c {
            0..=127 => Op::SetChar(c as u32),
            128..=131 => Op::SetChar(self.unsigned(c - 127)?),
            132 => Op::SetRule {
                height: self.i32()?,
                width: self.i32()?,
            },
            133..=136 => Op::PutChar(self.unsigned(c - 132)?),
            137 => Op::PutRule {
                height: self.i32()?,
                width: self.i32()?,
            },
            138 => Op::Nop,
            139 => {
                let mut counts = [0i32; 10];
                for c in counts.iter_mut() {
                    *c = self.i32()?;
                }
                Op::Bop {
                    counts,
                    prev: self.i32()?,
                }
            }
            140 => Op::Eop,
            141 => Op::Push,
            142 => Op::Pop,
            143..=146 => Op::Right(self.signed(c - 142)?),
            147 => Op::W0,
            148..=151 => Op::W(self.signed(c - 147)?),
            152 => Op::X0,
            153..=156 => Op::X(self.signed(c - 152)?),
            157..=160 => Op::Down(self.signed(c - 156)?),
            161 => Op::Y0,
            162..=165 => Op::Y(self.signed(c - 161)?),
            166 => Op::Z0,
            167..=170 => Op::Z(self.signed(c - 166)?),
            171..=234 => Op::Font((c - 171) as u32),
            235..=238 => Op::Font(self.unsigned(c - 234)?),
            239..=242 => {
                let n = self.unsigned(c - 238)? as usize;
                Op::Special(self.bytes(n)?.to_vec())
            }
            243..=246 => {
                let number = self.unsigned(c - 242)?;
                let checksum = self.u32()?;
                let scaled_size = self.i32()?;
                let design_size = self.i32()?;
                let a = self.u8()? as usize;
                let l = self.u8()? as usize;
                let area = String::from_utf8_lossy(self.bytes(a)?).into_owned();
                let name = String::from_utf8_lossy(self.bytes(l)?).into_owned();
                Op::FontDef(FontDef {
                    number,
                    checksum,
                    scaled_size,
                    design_size,
                    area,
                    name,
                })
            }
            247 => {
                let id = self.u8()?;
                let num = self.u32()?;
                let den = self.u32()?;
                let mag = self.u32()?;
                let k = self.u8()? as usize;
                Op::Pre {
                    id,
                    num,
                    den,
                    mag,
                    comment: self.bytes(k)?.to_vec(),
                }
            }
            248 => Op::Post,
            249 => Op::PostPost,
            251 if kind == Kind::Xdv => {
                let flags = self.u8()?;
                let mut matrix = [0i32; 6];
                for m in matrix.iter_mut() {
                    *m = self.i32()?;
                }
                let page = self.u16()?;
                let l = self.u16()? as usize;
                Op::PicFile {
                    flags,
                    matrix,
                    page,
                    path: self.bytes(l)?.to_vec(),
                }
            }
            252 if kind == Kind::Xdv => {
                let number = self.u32()?;
                let size = self.i32()?;
                let flags = self.u16()?;
                let l = self.u8()? as usize;
                let name = String::from_utf8_lossy(self.bytes(l)?).into_owned();
                let index = self.u32()?;
                let rgba = if flags & NativeFontDef::FLAG_COLORED != 0 {
                    let b = self.bytes(4)?;
                    Some([b[0], b[1], b[2], b[3]])
                } else {
                    None
                };
                let fixed =
                    |r: &mut Reader| -> Result<f64, DviError> { Ok(r.i32()? as f64 / 65536.0) };
                let extend = if flags & NativeFontDef::FLAG_EXTEND != 0 {
                    Some(fixed(self)?)
                } else {
                    None
                };
                let slant = if flags & NativeFontDef::FLAG_SLANT != 0 {
                    Some(fixed(self)?)
                } else {
                    None
                };
                let embolden = if flags & NativeFontDef::FLAG_EMBOLDEN != 0 {
                    Some(fixed(self)?)
                } else {
                    None
                };
                Op::NativeFontDef(NativeFontDef {
                    number,
                    size,
                    flags,
                    name,
                    index,
                    rgba,
                    extend,
                    slant,
                    embolden,
                })
            }
            253 | 254 if kind == Kind::Xdv => {
                let text = if c == 254 {
                    let l = self.u16()? as usize;
                    let mut t = Vec::with_capacity(l);
                    for _ in 0..l {
                        t.push(self.u16()?);
                    }
                    t
                } else {
                    Vec::new()
                };
                let width = self.i32()?;
                let k = self.u16()? as usize;
                let mut positions = Vec::with_capacity(k);
                for _ in 0..k {
                    positions.push((self.i32()?, self.i32()?));
                }
                let mut ids = Vec::with_capacity(k);
                for _ in 0..k {
                    ids.push(self.u16()?);
                }
                Op::SetGlyphs(Glyphs {
                    width,
                    positions,
                    ids,
                    text,
                })
            }
            255 if kind == Kind::Dvi => Op::Dir(match self.u8()? {
                0 => Direction::Yoko,
                1 => Direction::Tate,
                _ => Direction::Dtou,
            }),
            _ => return err(at, format!("undefined opcode {c}")),
        })
    }
}

/// 解析済みの DVI: preamble、各ページの命令列の位置、postamble のフォント定義
#[derive(Debug, Clone)]
pub struct Dvi<'a> {
    pub data: &'a [u8],
    pub kind: Kind,
    pub id: u8,
    pub num: u32,
    pub den: u32,
    pub mag: u32,
    pub comment: String,
    /// 各ページの `bop` の位置（ファイル順）
    pub pages: Vec<usize>,
    /// 最大ページ高さ・幅（sp）、スタックの最大深さ（postamble）
    pub max_height: i32,
    pub max_width: i32,
    pub max_stack: u16,
    pub fonts: Vec<FontDef>,
    pub native_fonts: Vec<NativeFontDef>,
    /// 縦組を含む pTeX の DVI（post_post の id が 3）
    pub has_tate: bool,
}

impl<'a> Dvi<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Dvi<'a>, DviError> {
        let mut r = Reader::new(data);
        let (id, num, den, mag, comment) = match r.op(Kind::Dvi)? {
            Op::Pre {
                id,
                num,
                den,
                mag,
                comment,
            } => (id, num, den, mag, comment),
            _ => return err(0, "missing preamble"),
        };
        let kind = match id {
            2 => Kind::Dvi,
            7 => Kind::Xdv,
            5 | 6 => return err(1, format!("old XDV id {id} is not supported")),
            _ => return err(1, format!("unknown DVI id {id}")),
        };
        // postamble: 末尾の 223 の詰め物 → id → post の位置
        let mut end = data.len();
        while end > 0 && data[end - 1] == 223 {
            end -= 1;
        }
        if end < 5 {
            return err(end, "truncated postamble");
        }
        let post_id = data[end - 1];
        let has_tate = post_id == 3;
        let post_ptr =
            u32::from_be_bytes([data[end - 5], data[end - 4], data[end - 3], data[end - 2]])
                as usize;
        if post_ptr >= data.len() || data[post_ptr] != 248 {
            return err(post_ptr, "post pointer does not point at post");
        }
        let mut r = Reader::new(data);
        r.pos = post_ptr + 1;
        let last_bop = r.i32()?;
        let _num = r.u32()?;
        let _den = r.u32()?;
        let _mag = r.u32()?;
        let max_height = r.i32()?;
        let max_width = r.i32()?;
        let max_stack = r.u16()?;
        let _total_pages = r.u16()?;
        let mut fonts = Vec::new();
        let mut native_fonts = Vec::new();
        loop {
            match r.op(kind)? {
                Op::FontDef(f) => fonts.push(f),
                Op::NativeFontDef(f) => native_fonts.push(f),
                Op::Nop => {}
                Op::PostPost => break,
                other => return err(r.pos, format!("unexpected {other:?} in postamble")),
            }
        }
        // ページは post の last_bop から逆向きに辿る
        let mut pages = Vec::new();
        // ページは post の last_bop から逆向きに辿る。prev は必ず手前を指す（同じ位置や後ろを指せば循環）
        let mut p = last_bop;
        let mut prev_at = usize::MAX;
        while p >= 0 {
            let at = p as usize;
            if at >= data.len() || data[at] != 139 {
                return err(at, "bop pointer does not point at bop");
            }
            if at >= prev_at {
                return err(at, "bop chain does not go backwards (cycle)");
            }
            if pages.len() >= _total_pages as usize {
                return err(at, "more bop pointers than the postamble's page count");
            }
            pages.push(at);
            prev_at = at;
            let mut rr = Reader::new(data);
            rr.pos = at + 1 + 40;
            p = rr.i32()?;
        }
        pages.reverse();
        Ok(Dvi {
            data,
            kind,
            id,
            num,
            den,
            mag,
            comment: String::from_utf8_lossy(&comment).into_owned(),
            pages,
            max_height,
            max_width,
            max_stack,
            fonts,
            native_fonts,
            has_tate,
        })
    }

    /// 1 sp あたりの bp（PostScript ポイント）。DVI の num / den と mag から
    pub fn bp_per_sp(&self) -> f64 {
        // num / den は sp → 10^-7 m。1 in = 254000 単位、1 bp = 1/72 in
        let units_per_sp = self.num as f64 / self.den as f64 * self.mag as f64 / 1000.0;
        units_per_sp / 254e3 * 72.0
    }

    /// ページ `i` の命令列（`bop` の次から `eop` まで）
    pub fn page_ops(&self, i: usize) -> Result<Vec<Op>, DviError> {
        let mut r = Reader::new(self.data);
        r.pos = self.pages[i];
        let mut ops = Vec::new();
        match r.op(self.kind)? {
            Op::Bop { .. } => {}
            _ => return err(self.pages[i], "page does not start with bop"),
        }
        loop {
            let op = r.op(self.kind)?;
            if op == Op::Eop {
                break;
            }
            ops.push(op);
        }
        Ok(ops)
    }

    pub fn font(&self, number: u32) -> Option<&FontDef> {
        self.fonts.iter().find(|f| f.number == number)
    }

    pub fn native_font(&self, number: u32) -> Option<&NativeFontDef> {
        self.native_fonts.iter().find(|f| f.number == number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_three_byte_values_sign_extend() {
        let mut r = Reader::new(&[0xFF, 0xFF, 0xFE, 0x00, 0x00, 0x02]);
        assert_eq!(r.signed(3).unwrap(), -2);
        assert_eq!(r.signed(3).unwrap(), 2);
    }

    #[test]
    fn decodes_the_common_opcodes() {
        let bytes = [
            65u8, 128, 200, 141, 143, 0xF6, 142, 171, 239, 3, b'a', b'b', b'c', 157, 0x10,
        ];
        let mut r = Reader::new(&bytes);
        assert_eq!(r.op(Kind::Dvi).unwrap(), Op::SetChar(65));
        assert_eq!(r.op(Kind::Dvi).unwrap(), Op::SetChar(200));
        assert_eq!(r.op(Kind::Dvi).unwrap(), Op::Push);
        assert_eq!(r.op(Kind::Dvi).unwrap(), Op::Right(-10));
        assert_eq!(r.op(Kind::Dvi).unwrap(), Op::Pop);
        assert_eq!(r.op(Kind::Dvi).unwrap(), Op::Font(0));
        assert_eq!(r.op(Kind::Dvi).unwrap(), Op::Special(b"abc".to_vec()));
        assert_eq!(r.op(Kind::Dvi).unwrap(), Op::Down(16));
    }

    #[test]
    fn bp_per_sp_is_the_tex_ratio() {
        let dvi = Dvi {
            data: &[],
            kind: Kind::Dvi,
            id: 2,
            num: 25400000,
            den: 473628672,
            mag: 1000,
            comment: String::new(),
            pages: vec![],
            max_height: 0,
            max_width: 0,
            max_stack: 0,
            fonts: vec![],
            native_fonts: vec![],
            has_tate: false,
        };
        // 1 bp = 65536 × 72.27 / 72 sp
        let expected = 1.0 / (65536.0 * 72.27 / 72.0);
        assert!((dvi.bp_per_sp() - expected).abs() < 1e-15);
    }
}
