# SabiDVI 設計 v0

## 位置づけ

SabiDVI は SabiRender の入口の一つ。DVI（Knuth の TeX、pTeX / upTeX の拡張）と XDV（XeTeX）を読み、
dvipdfmx 方言の `\special` を解釈して、装置非依存の描画命令列（`sabirender-display`）に変換する。
描画そのものは SabiRender の後端が行い、フォントの読み取りは SabiFace が行う。

対象の方言は dvipdfmx（`pdf:`、`color`、`x:`、`papersize`）に限る。dvips の `ps:` は対象外として記録だけする。

## 三クレート

| クレート | 内容 | 出典 |
|---|---|---|
| `sabidvi-format` | DVI / XDV の構文。preamble・postamble、ページの位置、命令の復号 | tex.web §583〜§600、`ptex-base.ch`（`dirchg` = 255）、`xetex.web`（251〜254、id = 7） |
| `sabidvi-special` | `\special` の解釈。`pdf:` の全キーワード表、寸法と変換（`transform_info`）、色（dvips の 68 色を含む） | dvipdfmx `spc_pdfm.c`、`spc_util.c`、`spc_xtx.c`、`specials.c` |
| `sabidvi-page` | ページの実行。位置の計算、文字・規則・special を描画命令列に | tex.web §585（h, v, w, x, y, z）、dvipdfmx `dvi.c`、`spc_pdfm.c` |
| `sabidvi-fonts` | `FontSource` の実装。フォント名の解決（VF → map → kanjix.map → 同名 pfb）、VF の合成、Type1 / OpenType の字形 | dvipdfmx `vf.c`、`fontmap.c`、SabiFace |
| `sabidvi-cli` | `sabidvi render`: 1 ページを PNG（参照ラスタライザ）または SVG（`sabirender-svg`）に。`--crop` でインクの範囲に切り詰める | |

## 座標

- DVI の (h, v) は sp。原点はページ左上から 1 in 内側、v は下向き。
- dvipdfmx と同じく紙面左上から (72 bp, 72 bp) の点を DVI の原点に置く。紙面は `papersize` / `pdf:pagesize` で変わる（既定は A4。dvipdfmx.cfg の `p` に相当）。
- ページ空間（bp、原点左下、y 上向き）への写像: x = 72 + h·k、y = H − 72 − v·k（k = bp/sp、H = 紙面高さ）。
- 縦組（pTeX の `dirchg` 1）: `right` は v を増やし、`down` は h を減らす（dvipdfmx `dvi.c`）。規則は幅と高さを入れ替えて置く。
  字形の回転は v0 では近似。

## special の意味論（dvipdfmx `spc_pdfm.c` に従う）

| special | 動作 |
|---|---|
| `pdf:code` | 現在の図形状態のまま演算子を流す。座標は `bcontent` が積んだ原点から |
| `pdf:literal` | 現在位置へ平行移動して流し、戻す（`q`/`Q` を使わない）。`direct` なら移動しない |
| `pdf:bcontent` / `econtent` | `q` と現在位置への平行移動を積む。`econtent` で `Q` の後に色スタックの色を再設定 |
| `pdf:btrans` / `etrans` | `q` と、現在位置を中心とする変換（`matrix`、または `rotate` → `scale`）。`etrans` で `Q` と色の再設定 |
| `pdf:bcolor` / `ecolor` / `scolor`、`color push` / `pop` | 色スタック（塗りと線） |
| `pdf:bxobj` … `exobj` / `uxobj` | 描画を捕まえて名前に結びつけ、`uxobj` で参照点を現在位置へ移して置く。`width` / `height` は境界箱に合わせた拡大 |
| `pdf:obj` / `put` | `<< /名前 << /ca /CA /LW >> >>` を拡張図形状態の資源として集める（PGF の不透明度） |
| `pdf:image` | 未対応（記録） |
| 注釈・しおり・文書情報・名前・mapline | 描画に関わらないので無視 |

`q`/`Q` の入れ子は special の境界をまたぐので、評価器はページの間ずっと一つを使い回す。ページの終わりで `finish` が対応しない `q` を閉じる。

## フォント

`FontSource` を通じて受け取る:

- 文字幅（TFM の `fix_word`、デザインサイズに対する比）。位置の計算に必須
- 字形の輪郭（字形単位）と字形単位から em への比。無ければ数えて報告する
- XDV のネイティブフォントは字形 ID で直接引く

TFM / JFM / VF / Type1 / OpenType の読み取りは SabiFace。`sabidvi-fonts` が VF の DVI 断片を再帰的に実行して部品の字形を一つの輪郭に合成する
（座標は 1000 / em、`right` などの寸法は fix_word × 実サイズ）。

## 検証

1. **構文**: TeX Live の `tex`、`uptex`、`xetex -no-pdf` で生成した DVI / XDV を解析する（`crates/sabidvi-format/tests/engines.rs`）。
2. **幾何の同一性**: 同じ DVI を `dvipdfmx -z0` で PDF にし、その内容ストリームを SabiRender の評価器で描画命令列に読み直して、
   SabiDVI の出力と経路・色・線幅・破線を数値で比べる（`crates/sabidvi-page/tests/pgf_dvipdfmx.rs`）。許容誤差は 2e-3 bp
   （dvipdfmx が小数 3〜5 桁で書くため）。TikZ の線・矩形・破線・丸め角・円・不透明度・クリップを含む。
3. **文字と規則の位置**: TFM の幅からの位置を plain TeX の出力で確かめる。
4. **字形**: Computer Modern（Type1）、Times（VF → Type1）、Latin Modern（XDV ネイティブ）、upTeX の和文（VF → kanjix.map → OpenType）を
   実際に描いてインクの位置を確かめる（`crates/sabidvi-fonts/tests/glyphs.rs`）。

エンジンやツールが無い環境ではテストを飛ばす。

## 今後

- `pdf:image`（PDF / PNG / JPEG）。
- PGF マニュアル全体の幾何の同一性（ページごとの回帰）。
- wasm 化と web-mathdb への受け渡し。
