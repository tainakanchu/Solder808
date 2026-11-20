# Solder808

808 系ドラム（BD / SD / HH / クラップ / カウベル）の回路をブラウザ内で 1 サンプルずつ解いて鳴らす試作。部品の許容誤差・経年劣化・温度を「個体」として変えられる。

https://tainakanchu.github.io/Solder808/

最初の画面のジャンル別プリセットを押すとすぐ鳴る。プリセットは 2 小節で、2 小節目の最後 4 マスがおかず（1 小節にも切り替えられる）。アレンジ（定番を崩さない範囲の変化）、個体ガチャ（製造年・状態・置き場所で名前のついた個体を引く）、共有リンク（パターン・テンポ・個体・ノブを URL に入れる）がある。

## MIDI クロック同期

再生ボタンの並びの「同期」で MIDI 入力を選ぶと、その入力の MIDI クロック（24 PPQN）・スタート／ストップ／コンティニュー・ソングポジションに合わせて鳴る。Web MIDI が使えるブラウザ（Chrome / Edge 系）だけで出る。

- Mac で同じマシンの DAW と合わせるときは、Audio MIDI 設定で IAC ドライバを有効にし、DAW から IAC バスへクロックを送る。
- 音はクロックを受けた時刻から「遅延」の表示ぶん遅れて鳴る（出力遅延＋ジッタ吸収の 20 ms）。DAW 側の MIDI クロックの同期ディレイ（Ableton Live なら Preferences の Link/Tempo/MIDI にある MIDI Clock Sync Delay）をその値だけマイナスにすると揃う。
- スイングと連打は、クロックの間隔から測ったテンポで計算する。

## 使い方

```bash
nix develop --profile .nix-profile/dev   # GC で devShell が消えないようにする
cd web && pnpm install && pnpm dev
```

- `pnpm dev` / `pnpm build` は先に Rust を wasm32 向けにビルドして `web/src/assets/solder808.wasm` に置く。
- `pnpm lint`（oxlint）、`pnpm fmt`（oxfmt）、`pnpm typecheck`。
- `cd dsp && cargo test --release`。`engine::fuzz` はつまみ・部品・個体をランダムに変えて、出力が有限で無音時に Newton が上限に張り付かないことを確かめる。`cargo test --release long_play -- --ignored --nocapture` で PPAP を 10 分鳴らして変化を見る。
- `main` に push すると GitHub Actions が同じ devShell でビルドして GitHub Pages に公開する（`.github/workflows/pages.yml`）。
- ネイティブで WAV を書き出して測る: `cd dsp && cargo run --release --bin render -- ../renders`

## 構成

- `dsp/` Rust。`circuit.rs` が MNA ソルバ（コンデンサは台形則、ダイオードと飽和オペアンプは毎サンプル Newton 法）。`bd.rs` / `sd.rs` / `hh.rs` / `cp.rs` / `cb.rs` が各ボイスの回路、`blocks.rs` が共通の回路ブロック、`parts.rs` が個体差と経年変化。
- `web/` Vite + Preact + TypeScript（`preact/hooks` を直接使い、JSX は tsconfig の `jsxImportSource` で Preact に向ける）。`src/music/` にプリセット・アレンジ・個体ガチャ・共有 URL。`src/audio/processor.ts` が AudioWorklet で、wasm-bindgen なしの C ABI を直接呼ぶ。シーケンサもワークレット内でサンプル単位に動く。

## 回路モデルの範囲

- 回路として解いている: BD の bridged-T 共振・再生帰還（decay）・ダイオードによるピッチ降下・tone・出力カップリング、SD の 2 つの共振とノイズのフィルタとエンベロープ、HH のバンドパス・エンベロープ・チョーク・出力ハイパス、クラップのバンドパスと 2 つのエンベロープ、カウベルのバンドパスと速い・遅いエンベロープ。
- 式で近似している: HH の 6 個とカウベルの 2 個の矩形波発振器（周波数は R と C の部品値から計算）、ノイズ源、VCA の利得カーブ。
- 共通回路（部品表の「共通」タブ）: 15 V 電源（出力抵抗＋平滑コンデンサ）は各音色の消費電流で下がり、全オペアンプの振れ幅とトリガー電圧に効く。アクセント線は 1 本の出力抵抗を、同時に鳴った音色の入力抵抗で分け合うので、多く重なるほどアクセントが弱まる（BD 単独 0.749 → 全音同時 0.646）。ミキサーはレベル用ボリューム（分圧）→ 47 kΩ → 反転加算オペアンプ → 出力コンデンサ。分圧器は等価なノートン電源に置き換えて解いている。
- 電源の電圧降下は全音同時でも約 2 %（14.7 V）と小さい。部品表で R1 supply output を大きくすると効きが分かりやすい。
- 部品定数は 808 の信号の流れに合わせた代表値で、サービスマニュアルの回路図とは突き合わせていない。
- 連立方程式は回路の形から決まる疎な消去手順を使い回して解く（ピボットが怪しいときだけ密な部分ピボットに戻って手順を作り直す）。6 音すべてを毎ステップ鳴らす極端な負荷で、ネイティブ実時間の約 3 倍。
- 画面の「負荷」は計算時間の割合。ワークレットに高精度タイマーがあればブロック 1 つの最大値（瞬間）も出す（Date.now しかない環境では 1 ms 刻みで不正確なので出さない）。途切れるときは詳細設定の「出力バッファ」を大きくできる。
- decay を最大にすると、部品のずれ方によっては BD が自己発振する個体がある（シード 40 個中 1 個程度）。

## 商標

Roland および TR-808 は Roland 株式会社の商標です。本プロジェクトは Roland とは関係ありません。
