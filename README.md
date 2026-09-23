# SabiDVI

SabiRender の入口。DVI（TeX、pTeX / upTeX）と XDV（XeTeX）を読み、dvipdfmx 方言の `\special` を解釈して、
装置非依存の描画命令列に変換する。描画は SabiRender、フォントの読み取りは SabiFace が行う。

| クレート | 内容 |
|---|---|
| `sabidvi-format` | DVI / XDV の構文。pTeX の縦組（`dirchg`）と XeTeX のネイティブフォント・字形列を含む |
| `sabidvi-special` | `\special` の解釈。dvipdfmx の `pdf:` 全キーワード、寸法と変換、色（dvips の色名を含む）、`x:`、`papersize` |
| `sabidvi-page` | ページの実行。位置の計算、文字・規則・special を SabiRender の描画命令列に変換する |
| `sabidvi-fonts` | `FontSource` の実装。`.vf`（仮想フォントの合成）、`pdftex.map` + `.enc` + Type1、`kanjix.map` + OpenType、XDV のネイティブフォントを SabiFace で読む。探索は `kpsewhich` |
| `sabidvi-cli` | `sabidvi render file.dvi -o out.png|out.svg [--dpi 150] [--crop [余白bp]]`。参照ラスタライザで PNG に、または経路のまま SVG にする。`--crop` はインクの範囲に切り詰め、`--meta out.json` でインクの範囲と基線（`special{sabidvi:mark baseline}` の位置）を書く（数式の埋め込み用） |

設計は `specification/design.md`。

## 検証の方針

- 構文は TeX Live の `tex` / `uptex` / `xetex` が出す DVI / XDV を実際に生成して解析する。
- 幾何の同一性は、同じ DVI を `dvipdfmx -z0` で PDF にし、その内容ストリームを SabiRender の評価器で読み直して数値で比べる。
  ラスタライズを介さないので、誤差の原因を切り分けられる。
- ツールが無い環境ではテストを飛ばす。

## 開発

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```
