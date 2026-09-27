# 契約 case 台帳

品質保証指針 `qa-v0`（`SabiSeries/qa/`）の [記録テンプレート](../../SabiSeries/qa/record-template.md) に対応する、このリポジトリの予定 case。
各 case はテスト関数 1 つに対応し、実行すると台帳（`target/qa-ledger/*.tsv`、または `SABI_QA_LEDGER`）に
PASS / FAIL / BLOCKED / NOT-RUN と比較件数を残す。`scripts/qa-ledger.sh` が本表の予定件数と突き合わせる。

`required` は oracle プロファイルで必須（無ければ BLOCKED で失敗）。`optional` は追加のエンジン・フォントに依り、
`SABI_STRICT_OPTIONAL` を設定したときだけ失敗にする。任意かどうかはこの表と `Case::optional` の宣言で決め、理由の文字列から推測しない。
参照ツールが起動したのに DVI / PDF を出さなければ FAIL（環境不足ではなく検査の失敗）。

| case | 契約 | プロファイル | 必須 | 参照資源 | 内容 |
|---|---|---|---|---|---|
| DVI-ENGINE-TEX | C-DVI | dvi-core | required | tex、cmr10 | Knuth の TeX の DVI: フォント定義、文字、special、規則 |
| DVI-ENGINE-UPTEX | C-DVI | dvi-japanese | optional | uptex、upjisr-v | 縦組の `dirchg` と後付けの id 3 |
| DVI-ENGINE-XETEX | C-DVI | xdv-native | optional | xetex、lmroman10-regular.otf | XDV のネイティブフォント定義と `set_glyphs` |
| DVI-GLYPH-CM | C-FONT, C-DVI | font-type1 | required | tex、cmr10.tfm/.pfb | Type1 字形の描画とインクの位置 |
| DVI-GLYPH-TIMES | C-FONT | font-type1 | optional | ptmr7t.vf、utmr8a.pfb | 仮想フォントの合成と字形の行列 |
| DVI-GLYPH-XETEX | C-FONT, C-DVI | xdv-native | optional | xetex、lmroman10-regular.otf | ネイティブ字形の描画 |
| DVI-GLYPH-UPTEX | C-FONT, C-DVI | dvi-japanese | optional | uptex、upjisr-h.vf、kanjix.map、原ノ味 | VF → kanjix.map → OpenType |
| DVI-FONT-OTMAP | C-FONT | font-opentype | optional | lmroman10-regular.otf | pdftex.map の SlantFont / ExtendFont が OpenType の字形の行列に掛かる |
| DVI-PGF-DVIPDFMX | C-DVI, C-DRAW | dvi-core, render-geometry | required | etex、PGF、dvipdfmx、cmr10.tfm | TikZ の図を dvipdfmx の PDF と幾何で照合（2e-3 bp） |
| DVI-TFM-POSITION | C-DVI, C-FONT | dvi-core | required | tex、cmr10.tfm | 文字と規則の位置を TFM の幅と手計算で確認 |

外部資源に依らない検査（手計算の期待値による紙面・変換・組方向・bop の循環・字形の変換、wasm の失敗と部分結果、
special の解釈）は `cargo test` の単体テストとして常に実行し、台帳には載せない。

## 参照資源のロック

oracle プロファイルの基準は TeX Live 2025 の配布物。CI は Ubuntu の `texlive-binaries` / `texlive-base` /
`texlive-fonts-recommended` / `texlive-pictures`（中核）と、`texlive-lang-japanese` / `fonts-haranoaji` / `fonts-lmodern`（任意）を導入する
（`.github/workflows/ci.yml`）。版と digest の台帳化は段階 1 の残件。
