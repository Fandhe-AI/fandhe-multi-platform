//! 機械学習モデル推論の抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/ml-inference.json`、親 #89、Issue #289）。
//!
//! 扱う範囲は「固定の線形モデル `y = 2·x0 + 3·x1 + 1` に固定入力 `x = [4, 5]` を渡して
//! 1 回推論し、出力を期待値（24）と照合する」ことだけ。モデルファイルは置かず、実行時に
//! バイト列をコードで組む（[`coreml_spec`] と Windows 用の [`onnx_model`]。`media_playback::demo_wav` と同じ発想）。
//! 画像の前処理・分類モデル・モデルの動的取得・利用者が持ち込むモデル・GPU / NPU の選択・
//! 推論の取り消しは扱わない。対象は macOS / iOS（Core ML）、Windows（WinML）、
//! Android（NNAPI）。Web は対象外。
//! 一発呼び出し型の同期 API。呼び出し側（`fandhe-egui-proxy` の effect ドライバ）は
//! UI を止めないようワーカースレッドから呼ぶ。
//!
//! # 契約
//!
//! - **成功**: 期待値と許容差（[`check_output`]）の中で一致した `y` だけ。外れた値・NaN・無限大は
//!   成功にしない（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **失敗の区別**: 非対応 OS・OS が古い → `Err(Unsupported("ml_inference"))`、OS がモデルを
//!   拒否 → `Err(Internal(`[`INVALID_MODEL`]`))`、期限切れ → `Err(Internal(`[`TIMEOUT`]`))`、
//!   出力の不一致 → `Err(Internal(`[`MISMATCH`]`))`、入力が有限値でない → `Err(InvalidInput)`、
//!   それ以外 → `Err(Internal)`
//! - **取り消しと権限は存在しない**: 推論は短い一発呼び出しで利用者が取り消す操作を持たず、
//!   OS の権限も要らない。キャンセル・権限拒否の失敗系は無い（契約表 10 の三値は成功 / 失敗のみ）。
//!   「壊れたモデルで試す」は期待どおりの失敗だが、画面では失敗として表示し成功に見せない
//! - **期限**（契約表 3）: 期限は Rust のドライバだけが持つ（[`INFER_DEADLINE`]）。起点は要求を
//!   受け付けた時の単調時計 `Instant`。実装へは残り時間 `remaining` を渡し、Apple はその分だけ
//!   待つ。Windows / NNAPI は OS 側で中断できず `remaining` を使わない。期限後に届いた結果は
//!   成功にしない（ドライバが捨てる）。資源は結果を捨てる前に解放する
//! - **並行性**（契約表 2）: 世代は画面側が要求ごとに進め、完了は「処理中かつ同じ世代」の
//!   ときだけ適用する。処理中の再押下は無視する
//! - **ライフサイクル**（契約表 1）: 推論は 1 回で短いため背景遷移でも中断しない。復帰後に届いた
//!   結果も世代照合を通れば表示する
//!
//! # 守る範囲と守らない範囲
//!
//! 守る範囲: 1 プロセス内での要求と完了の順序、嘘の成功の禁止、資源の解放。守らない範囲: OS
//! ごとの所要時間の違い（初回読み込みのキャッシュを含む）、期限後も OS の呼び出しが走り切る
//! こと、複数プロセス。入力・出力・モデルのバイト列・OS のエラー文はログへ出さない。

use std::time::Duration;

use crate::DeviceError;

/// 線形モデルの重み `w`。
pub const WEIGHTS: [f64; 2] = [2.0, 3.0];
/// 線形モデルのバイアス `b`。
pub const BIAS: f64 = 1.0;
/// 推論に渡す固定入力。期待値は `2·4 + 3·5 + 1 = 24`。
pub const DEMO_INPUT: [f32; 2] = [4.0, 5.0];
/// 要求を受け付けてから結果を確定するまでの期限（Rust のドライバだけが持つ）。
pub const INFER_DEADLINE: Duration = Duration::from_secs(10);

/// OS がモデルを拒否したときの `Internal` の識別子（SSOT）。
pub const INVALID_MODEL: &str = "ml_inference invalid_model";
/// 期限切れのときの `Internal` の識別子（SSOT）。
pub const TIMEOUT: &str = "ml_inference timeout";
/// 出力が期待値と合わないときの `Internal` の識別子（SSOT）。
pub const MISMATCH: &str = "ml_inference mismatch";

/// 推論に使うモデルの選択。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoModel {
    /// 正しい線形モデル。
    Valid,
    /// 末尾を切り詰めた壊れたモデル。OS が必ず拒否する。
    Corrupt,
}

/// 機械学習推論の契約（モジュール doc の「契約」節）。
pub trait MlInference: Send + Sync {
    /// `model` を読み込み `input` を 1 回推論して出力 `y` を返す。`remaining` は期限までの残り時間。
    fn infer(
        &self,
        model: DemoModel,
        input: [f32; 2],
        remaining: Duration,
    ) -> Result<f64, DeviceError>;
}

/// 対象外 OS（Linux 等）・未実装 OS のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedMlInference;

impl MlInference for UnsupportedMlInference {
    fn infer(&self, _: DemoModel, _: [f32; 2], _: Duration) -> Result<f64, DeviceError> {
        Err(DeviceError::Unsupported("ml_inference"))
    }
}

/// 入力から得る期待値 `w·x + b`。
pub fn expected(input: [f32; 2]) -> f64 {
    WEIGHTS[0] * f64::from(input[0]) + WEIGHTS[1] * f64::from(input[1]) + BIAS
}

/// 入力が有限値であることを OS を呼ぶ前に検査する。
pub fn validate_input(input: [f32; 2]) -> Result<(), DeviceError> {
    if input.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(DeviceError::InvalidInput("ml_inference input"))
    }
}

/// 出力 `y` を期待値と照合する。NaN・無限大・許容差（相対 1e-3、下限 1）の外は `Internal(MISMATCH)`。
pub fn check_output(y: f64, expected: f64) -> Result<f64, DeviceError> {
    if y.is_finite() && (y - expected).abs() <= 1e-3 * expected.abs().max(1.0) {
        Ok(y)
    } else {
        Err(DeviceError::Internal(MISMATCH))
    }
}

// --- protobuf の最小エンコーダ（Core ML 仕様と ONNX モデルの組み立て専用） ---

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// varint フィールド。
fn field_varint(out: &mut Vec<u8>, field: u64, v: u64) {
    varint(out, field << 3);
    varint(out, v);
}

/// length-delimited フィールド（文字列・入れ子メッセージ・packed）。
fn field_bytes(out: &mut Vec<u8>, field: u64, body: &[u8]) {
    varint(out, field << 3 | 2);
    varint(out, body.len() as u64);
    out.extend_from_slice(body);
}

fn packed_f64(values: &[f64]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// 線形モデルの Core ML 仕様（specificationVersion 1、GLMRegressor）。`Corrupt` は末尾を
/// 切り詰め、length-delimited の長さが合わず OS が逆シリアライズで必ず拒否する。
pub fn coreml_spec(model: DemoModel) -> Vec<u8> {
    // 入力 x: MultiArray(shape [2], DOUBLE = 0x10040)
    let mut array = Vec::new();
    field_varint(&mut array, 1, 2);
    field_varint(&mut array, 2, 0x10040);
    let mut x_type = Vec::new();
    field_bytes(&mut x_type, 5, &array);
    let mut x = Vec::new();
    field_bytes(&mut x, 1, b"x");
    field_bytes(&mut x, 3, &x_type);
    // 出力 y: Double
    let mut y_type = Vec::new();
    field_bytes(&mut y_type, 2, &[]);
    let mut y = Vec::new();
    field_bytes(&mut y, 1, b"y");
    field_bytes(&mut y, 3, &y_type);
    let mut desc = Vec::new();
    field_bytes(&mut desc, 1, &x);
    field_bytes(&mut desc, 10, &y);
    field_bytes(&mut desc, 11, b"y");
    // GLMRegressor: weights(1) / offset(2)
    let mut weights = Vec::new();
    field_bytes(&mut weights, 1, &packed_f64(&WEIGHTS));
    let mut glm = Vec::new();
    field_bytes(&mut glm, 1, &weights);
    field_bytes(&mut glm, 2, &packed_f64(&[BIAS]));
    let mut spec = Vec::new();
    field_varint(&mut spec, 1, 1);
    field_bytes(&mut spec, 2, &desc);
    field_bytes(&mut spec, 300, &glm);
    if model == DemoModel::Corrupt {
        spec.truncate(spec.len() - 8);
    }
    spec
}

fn packed_f32(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn message(fields: &[(u64, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (f, b) in fields {
        field_bytes(&mut out, *f, b);
    }
    out
}

/// ONNX TensorProto（FLOAT）。dims は 1 要素ずつ varint、float_data は packed。
fn onnx_tensor(dims: &[i64], data: &[f32]) -> Vec<u8> {
    let mut t = Vec::new();
    for d in dims {
        field_varint(&mut t, 1, *d as u64);
    }
    field_varint(&mut t, 2, 1);
    field_bytes(&mut t, 4, &packed_f32(data));
    t
}

/// ONNX ValueInfoProto（FLOAT テンソルの名前と形状）。
fn onnx_value_info(name: &str, dims: &[u64]) -> Vec<u8> {
    let mut shape = Vec::new();
    for d in dims {
        let mut dim = Vec::new();
        field_varint(&mut dim, 1, *d);
        field_bytes(&mut shape, 1, &dim);
    }
    let mut tensor_type = Vec::new();
    field_varint(&mut tensor_type, 1, 1);
    field_bytes(&mut tensor_type, 2, &shape);
    let ty = message(&[(1, &tensor_type)]);
    message(&[(1, name.as_bytes()), (2, &ty)])
}

/// ONNX NodeProto。`attr` は Constant の value 属性（TensorProto）。
fn onnx_node(op: &str, inputs: &[&str], outputs: &[&str], attr: Option<&[u8]>) -> Vec<u8> {
    let mut n = Vec::new();
    for i in inputs {
        field_bytes(&mut n, 1, i.as_bytes());
    }
    for o in outputs {
        field_bytes(&mut n, 2, o.as_bytes());
    }
    field_bytes(&mut n, 4, op.as_bytes());
    if let Some(t) = attr {
        let mut a = Vec::new();
        field_bytes(&mut a, 1, b"value");
        field_bytes(&mut a, 5, t);
        field_varint(&mut a, 20, 4); // AttributeType::TENSOR
        field_bytes(&mut n, 5, &a);
    }
    n
}

/// 線形モデルの ONNX（Windows の WinML 用）。`Y = MatMul(X, W) + B`、X は `[1,2]`、Y は `[1,1]`。
///
/// IR version 3 / opset 7 は Windows 10 1809 の WinML が読める上限。重みは initializer では
/// なく `Constant` ノードに持たせる（IR 3 の initializer は入力にも並べる必要があり、並べると
/// WinML の入力特徴に `W` / `B` が現れるため）。graph を最後のフィールドに置くので、`Corrupt`
/// は末尾を切り詰めると長さが合わず OS が逆シリアライズで必ず拒否する。
/// 実際に WinML が読めるかは Windows 実機確認待ち（#288）。
pub fn onnx_model(model: DemoModel) -> Vec<u8> {
    let w = onnx_tensor(&[2, 1], &WEIGHTS.map(|v| v as f32));
    let b = onnx_tensor(&[1], &[BIAS as f32]);
    let mut graph = Vec::new();
    field_bytes(&mut graph, 1, &onnx_node("Constant", &[], &["W"], Some(&w)));
    field_bytes(&mut graph, 1, &onnx_node("Constant", &[], &["B"], Some(&b)));
    field_bytes(
        &mut graph,
        1,
        &onnx_node("MatMul", &["X", "W"], &["XW"], None),
    );
    field_bytes(&mut graph, 1, &onnx_node("Add", &["XW", "B"], &["Y"], None));
    field_bytes(&mut graph, 2, b"linear");
    field_bytes(&mut graph, 11, &onnx_value_info("X", &[1, 2]));
    field_bytes(&mut graph, 12, &onnx_value_info("Y", &[1, 1]));
    let mut opset = Vec::new();
    field_varint(&mut opset, 2, 7);
    let mut m = Vec::new();
    field_varint(&mut m, 1, 3);
    field_bytes(&mut m, 8, &opset);
    field_bytes(&mut m, 7, &graph);
    if model == DemoModel::Corrupt {
        m.truncate(m.len() - 8);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOLDEN: &str = "0801121b0a0d0a01781a082a06080210c0800452070a01791a0212005a0179e2121e0a120a10000000000000004000000000000008401208000000000000f03f";

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn valid_spec_matches_golden_bytes() {
        assert_eq!(hex(&coreml_spec(DemoModel::Valid)), GOLDEN);
    }

    #[test]
    fn corrupt_spec_is_truncated_valid() {
        let (v, c) = (
            coreml_spec(DemoModel::Valid),
            coreml_spec(DemoModel::Corrupt),
        );
        assert_eq!(c, v[..v.len() - 8]);
    }

    #[test]
    fn varint_boundaries() {
        let mut b = Vec::new();
        varint(&mut b, 300);
        assert_eq!(b, [0xac, 0x02]);
        let mut b = Vec::new();
        varint(&mut b, 127);
        varint(&mut b, 128);
        assert_eq!(b, [0x7f, 0x80, 0x01]);
    }

    #[test]
    fn expected_is_24() {
        assert_eq!(expected(DEMO_INPUT), 24.0);
    }

    #[test]
    fn check_output_rejects_mismatch_nan_inf() {
        assert_eq!(check_output(24.0, 24.0).unwrap(), 24.0);
        assert_eq!(check_output(24.01, 24.0).unwrap(), 24.01);
        for bad in [0.0, 24.5, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(matches!(
                check_output(bad, 24.0),
                Err(DeviceError::Internal(MISMATCH))
            ));
        }
    }

    #[test]
    fn validate_input_rejects_non_finite() {
        assert!(validate_input(DEMO_INPUT).is_ok());
        for bad in [[f32::NAN, 1.0], [1.0, f32::INFINITY]] {
            assert!(matches!(
                validate_input(bad),
                Err(DeviceError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn unsupported_is_not_a_fake_success() {
        let e = UnsupportedMlInference
            .infer(DemoModel::Valid, DEMO_INPUT, INFER_DEADLINE)
            .unwrap_err();
        assert!(matches!(e, DeviceError::Unsupported("ml_inference")));
    }

    /// 入力の終端でぴったり終わる protobuf か（tag・varint・length-delimited を辿る）。
    fn well_formed(mut b: &[u8]) -> bool {
        fn rd(b: &mut &[u8]) -> Option<u64> {
            let (mut v, mut sh) = (0u64, 0);
            loop {
                let (&x, rest) = b.split_first()?;
                *b = rest;
                v |= u64::from(x & 0x7f) << sh;
                if x & 0x80 == 0 {
                    return Some(v);
                }
                sh += 7;
            }
        }
        while !b.is_empty() {
            let Some(tag) = rd(&mut b) else { return false };
            match tag & 7 {
                0 => {
                    if rd(&mut b).is_none() {
                        return false;
                    }
                }
                2 => {
                    let Some(n) = rd(&mut b) else { return false };
                    if n as usize > b.len() {
                        return false;
                    }
                    b = &b[n as usize..];
                }
                _ => return false,
            }
        }
        true
    }

    fn contains(h: &[u8], n: &[u8]) -> bool {
        h.windows(n.len()).any(|w| w == n)
    }

    #[test]
    fn onnx_model_structure() {
        let v = onnx_model(DemoModel::Valid);
        assert_eq!(&v[..2], [0x08, 0x03]);
        assert!(contains(&v, &[0x42, 0x02, 0x10, 0x07]));
        for s in ["Constant", "MatMul", "Add", "X", "W", "B", "Y"] {
            assert!(contains(&v, s.as_bytes()), "{s}");
        }
        let w: Vec<u8> = packed_f32(&[2.0, 3.0]);
        assert!(contains(&v, &w));
        assert!(contains(&v, &1.0f32.to_le_bytes()));
        assert!(well_formed(&v));
    }

    #[test]
    fn onnx_corrupt_is_truncated_and_malformed() {
        let (v, c) = (onnx_model(DemoModel::Valid), onnx_model(DemoModel::Corrupt));
        assert_eq!(c, v[..v.len() - 8]);
        assert!(!well_formed(&c));
    }
}
