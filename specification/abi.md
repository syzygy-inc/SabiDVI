# sabidvi-wasm の C ABI

`crates/sabidvi-wasm` が公開する関数と、その呼び方の約束。wasm-bindgen を使わず、線形メモリの受け渡しだけで動く
（SabiTeX の wasm と同じ流儀）。利用側の参照実装は web-mathdb の `webroot/public/sabidvi.js`。
契約は C-RESULT（結果の状態を明示）、C-JOB（ジョブの境界）、C-RESOURCE（資源の上限）。

## 関数

| 関数 | 引数 | 返り値 | 意味 |
|---|---|---|---|
| `sabidvi_alloc(len)` | `usize` | `*mut u8` | 線形メモリの確保。利用側はここに書いてから他の関数に渡す。`len = 0` でも有効な領域を返す |
| `sabidvi_free(ptr, len)` | `sabidvi_alloc` の返り値と同じ組 | – | 解放。組が違えば未定義 |
| `sabidvi_add_file(name_ptr, name_len, data_ptr, data_len)` | 名前（UTF-8）と内容 | `0` / `2`（名前が UTF-8 でない） | フォント束に加える。**内容は複製する**ので、呼び出し後に領域を `free` してよい。同じ名前は置き換え |
| `sabidvi_clear_files()` | – | – | フォント束を空にする（ジョブの境界） |
| `sabidvi_file_count()` | – | `usize` | 束のファイル数 |
| `sabidvi_page_count(dvi_ptr, dvi_len)` | DVI | ページ数、不正なら `-1` | DVI の解析だけ行う |
| `sabidvi_render(dvi_ptr, dvi_len, page, scale, margin)` | DVI、1 始まりのページ、bp あたり画素数、余白 bp | `0` 成功 / `1` 部分 / `2` 誤り | 1 ページを描く |
| `sabidvi_pixels_ptr()` / `sabidvi_pixels_len()` | – | RGBA8（プリマルチプライしない） | 直前の `render` の画像。誤りのときは長さ 0 |
| `sabidvi_meta_ptr()` / `sabidvi_meta_len()` | – | UTF-8 の JSON | 直前の `render` の付帯情報 |

`render` の返り値と meta:

- `0`: `width`、`height`、`scale`、`margin`、インクの範囲 `xmin`〜`ymax`（bp）、`baseline`（bp。`sabidvi:mark baseline` が
  無ければ紙面上端から 72 bp）、画像左下のページ座標 `originX` / `originY`、`chars`、`missingGlyphs`、`unsupported`（診断の文字列）、
  `missing`（探して見つからなかったファイル名。空）。
- `1`: 同じ形の meta。`missing` に見つからなかったファイル名（`pdftex.map`、`cmr10.tfm` など SabiDVI が探した名前そのまま）。
  画像は字形の欠けたもので、利用側は `missing` を取り寄せて `add_file` し、もう一度 `render` する（missing-file 方式）。
  取り寄せられないものがあれば、その画像を最終結果としてよい。
- `2`: `{"error": "..."}`。画像は空。

## 引数の範囲（C-RESOURCE）

- `page` は `1..=page_count`。範囲外は `2`。
- `scale` は有限で `0 < scale < 1000`。`margin` は有限で `0 <= margin`。範囲外は `2`（NaN を通すと 1×1 の画像と壊れた JSON になるので、
  ここで止める）。
- 画像の画素数は `sabirender_raster::MAX_PIXELS`（2^24）まで。超えると確保せずに `2`（`image too large`）。
- DVI の解析誤り、ページ実行の誤りは `2`。フォントの欠落は誤りではなく `1`。

## ポインタの寿命

- `sabidvi_alloc` の領域は利用側のもの。ライブラリは保持しない（`add_file` は複製、`render` / `page_count` は呼び出しの間だけ読む）。
  `sabidvi_free` で返す。
- `pixels_ptr/len` と `meta_ptr/len` はライブラリ所有のバッファを指し、**次の `sabidvi_render` まで**有効。`render` は成功・部分・誤りの
  いずれでも両方を書き換える（誤りでは画像を空にする。前回の画像が残ることはない）。
- メモリを確保する呼び出し（`alloc`、`add_file`、`render`）は線形メモリを伸ばすことがあり、JS 側の `ArrayBuffer` は
  伸長で切り離される。**関数を呼ぶたびに `memory.buffer` を読み直してから** `Uint8Array` を作る（`sabidvi.js` はそうしている）。
  保持していた `Uint8Array` を再利用しない。

## ジョブの境界（C-JOB）

- 状態はフォント束・直前の画像・直前の meta の 3 つで、いずれも wasm インスタンス（スレッド）ごと。再入不可。
- フォント束は `render` をまたいで持続する（取り寄せて呼び直す方式のため）。文書や利用者が変わるときは `clear_files` で
  新しいジョブにする。束の内容が結果に影響するのはフォントの解決だけで、DVI の解釈やページの状態は毎回作り直す。
- `render` の結果はその呼び出しのもの。別ページを描けば置き換わり、前のページに戻しても同じ結果になる
  （`each_render_replaces_the_image_and_meta_for_the_requested_page`）。

## 検査

`crates/sabidvi-wasm/src/lib.rs` の単体テスト（外部資源に依らず常に実行）: 罫線の描画と基線、不正な入力・ページ・scale・margin が
`2` で画像が空、部分結果の一貫性と `missing` の順、束の持続と `clear_files`、ページ間で状態が漏れないこと。
web-mathdb 側は `packages/typeset/tests/wasm.test.ts` が公開フォント束と実際の DVI で同じ手順を Node から通す。
