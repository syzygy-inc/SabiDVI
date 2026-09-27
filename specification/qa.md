# SabiDVI 内部品質保証

策定: 2026-09-24。参照設計版: `qa-v0`。系列 [契約](../../SabiSeries/qa/contracts.md) の C-DVI / C-FONT / C-DRAW / C-JOB / C-RESULT / C-RESOURCE に対応する。境界の単位・意味は外側で規定し、実行器・解決器の状態の保ち方はここで扱う。

## 内部不変条件

| 領域 | 維持する条件 | 検査 |
|---|---|---|
| DVI/XDV parser | 長さ・ポインタ・opcode・ページ連鎖が妥当で有限 | 正常連鎖、自己/複数循環、切断、count不一致、invalid page |
| ページ位置 | push/pop 対象の位置・組方向を同時に復元 | 横/縦の往復、入れ子、stack下溢れ、方向別移動 |
| 紙面 | 文書/ページの指定と既定・override の優先規則を一貫適用 | 複数ページ、先頭/末尾special、PNG/SVG/meta一致 |
| special 状態 | q/Q、bcontent、btrans、color、clip の各責任を分離 | scope の内外、原点二重適用、未対応診断 |
| 字形・罫線・mark | ページ位置と実効変換を同じ規則で適用 | scale/rotate、TFM/native、baseline、VF部品 |
| font 解決 | 取得失敗の原因と変形を保持、資源更新で古い cache を使わない | Type1/OpenType map、同名差替え、欠落の再供給 |
| VF 展開 | depth・操作数・参照先・packet長を検査 | 自己/相互参照、深さ超過、部分欠落 |
| CLI/wasm | 実効紙面、status、meta と画像の要求一致 | 完全/部分/失敗、連続呼出し、無効scale/margin、画素上限 |

内部 state の full snapshot を系列の期待値にしない。最小 DVI を public parse→execute の境界から入れ、ページ空間へ正規化した観測値で比較する。DVI 方言で対象外の状態を push/pop に含めてしまう誤りも負例で捕える。

## 既存基盤と追加する検査

`sabidvi-page/tests/synthetic.rs` の D1〜D4、実エンジンの DVI/XDV、PGF と dvipdfmx の比較を維持する。文字・native 字形にも変換を掛ける回帰、OpenType map の変形、複数ページの紙面、missing width と glyph が別々に欠ける入力を追加候補にする。

dvipdfmx PDF と自分の出力を同じ SabiRender で読む比較には共通故障がある。手計算の synthetic と、外側の独立レンダラ比較を併用する。外側で semantic equality を要求するため、display list の項目分割や字形配置の内部型は変更できる。

```sh
cargo test --workspace
# oracle では子プロセス環境に SABI_STRICT_TESTS=1
# 宣言するoptional群も必須にする検証では SABI_STRICT_OPTIONAL=1 も設定
```

外部コマンドの起動だけでなく終了コード、生成ファイル、予定した比較の完了まで確認する。optional は case の属性にし、欠落理由の文字列による推測を品質基準の正本にしない。

## wasm と統合ホスト

現行の render 呼出し、file bundle、pixels/meta pointer の所有権・期限は ABI 文書に固定する。render 失敗で古い画像が残らないこと、Partial の返却で missing/unsupported と画像の版が一致することを確認する。単なる thread-local 状態をジョブ隔離の保証と解釈しない。

ホストの取得順・遅延・キャンセルは系列 C-JOB。SabiDVI 側は「同じ入力束なら同じ結果」「追加資源が次回に反映」「一回の結果は混ざらない」という境界を検証する。unsafe pointer の契約違反とバイト列の構文不正を区別し、後者は通常エラーにする。

処理時間・画素数・最大packet等の上限を確保前に確認する。Worker での timeout はホスト担当。library が途中停止に対応していないなら、その制限を文書化する。

## 形式化と依存変更

Lean の候補は座標変換の有理数モデル、方向付き移動、stack の保存対象。実際の丸め・整数域・float 変換は別の対応条件にする。TLA+ はホストの資源取得 protocol と結ぶ観測イベントを定義するために使い、DVI の全opcodeを分散状態機械にしない。

Face/Render の git revision 更新、glyph API、display list の変更時は依存の候補組で C-FONT/C-DRAW の adapter を検証する。現在の固定依存を勝手に隣の path に置換してテストを通しただけでは、配布構成の証拠にならない。

## 当面の完了条件

strict の早期終了経路、OpenType map の変形、partial/失敗時の外部状態を先に閉じる。既存の synthetic は契約 ID と対応づけて維持する。Lean/TLA+ の計画は [系列方針](../../SabiSeries/qa/formal-methods.md) に従い、モデル検査済みとはまだ宣言しない。
