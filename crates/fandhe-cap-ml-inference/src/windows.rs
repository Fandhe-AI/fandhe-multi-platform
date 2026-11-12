//! Windows 向け機械学習推論（Issue #290）。WinML（`Windows.AI.MachineLearning`、Windows 10 1809 以降）を呼ぶ。
//!
//! 呼び出し元: `driver.rs` の Windows 経路。ワーカースレッド上で `infer` を同期に
//! 呼ぶ。期限・世代照合・出力の照合・画面向けの失敗写像は呼び出し側が担い、本ファイルは
//! 「ブロックしてよい同期関数」だけを持つ。契約の SSOT は `crate::contract`。
//! モデルは `onnx_model` が実行時に組んだバイト列をメモリ上のストリームで渡す（一時ファイルなし）。
//!
//! 失敗の写像（全経路で一貫）:
//! - `LearningModelDevice::Create` が `REGDB_E_CLASSNOTREG`（WinML が無い OS）→ `Unsupported`。
//!   WinML の無い OS で「モデル拒否」と偽らないよう、デバイス生成をモデル読み込みより先に行う
//! - `LoadFromStream` / `CreateFromModelOnDevice` の失敗（OS によるモデルの拒否）→ `Internal(INVALID_MODEL)`
//! - それ以外（ストリーム・束縛・評価・出力取得の失敗、出力が 1 要素でない）→ `Internal`（固定ラベルのみ。
//!   HRESULT・`ErrorStatus` はログにも画面にも出さない。0.0 へ畳まない）
//!
//! `RoInitialize` は呼ばず（MTA フォールバック）、`unsafe` は使わない。計算は CPU 固定。
//! `remaining` は使わない（同期呼び出しは OS 側で中断できず、期限後も走り切る。契約の「守らない範囲」）。
//!
//! 既知の天井: unpackaged exe からの動作と 1809 での ONNX の読み込みは未確認で実機確認待ち（#288）。
//! 型検査は `x86_64-pc-windows-gnu` のみ。

use std::sync::Arc;
use std::time::Duration;

use crate::contract::{DemoModel, INVALID_MODEL, MlInference, onnx_model};
use crate::error::MlInferenceError;
use windows::AI::MachineLearning::{
    LearningModel, LearningModelBinding, LearningModelDevice, LearningModelDeviceKind,
    LearningModelSession, TensorFloat,
};
use windows::Storage::Streams::{
    DataWriter, InMemoryRandomAccessStream, RandomAccessStreamReference,
};
use windows::Win32::Foundation::REGDB_E_CLASSNOTREG;
use windows::core::{HSTRING, IInspectable, Interface};

/// Windows の機械学習推論（WinML）。
pub(crate) struct WindowsMlInference;

fn internal(label: &'static str) -> MlInferenceError {
    MlInferenceError::Internal(label)
}

/// モデルのバイト列をメモリ上のストリーム参照にする。
fn stream_of(bytes: &[u8]) -> Result<RandomAccessStreamReference, MlInferenceError> {
    let e = |_| internal("ml_inference stream");
    let stream = InMemoryRandomAccessStream::new().map_err(e)?;
    let writer = DataWriter::CreateDataWriter(&stream).map_err(e)?;
    writer.WriteBytes(bytes).map_err(e)?;
    writer.StoreAsync().map_err(e)?.get().map_err(e)?;
    // drop 時に stream を閉じさせない。
    writer.DetachStream().map_err(e)?;
    stream.Seek(0).map_err(e)?;
    RandomAccessStreamReference::CreateFromStream(&stream).map_err(e)
}

impl MlInference for WindowsMlInference {
    fn infer(
        &self,
        model: DemoModel,
        input: [f32; 2],
        _remaining: Duration,
    ) -> Result<f64, MlInferenceError> {
        let device = LearningModelDevice::Create(LearningModelDeviceKind::Cpu).map_err(|e| {
            if e.code() == REGDB_E_CLASSNOTREG {
                MlInferenceError::Unsupported
            } else {
                internal("ml_inference device")
            }
        })?;
        let reference = stream_of(&onnx_model(model))?;
        let rejected = |_| MlInferenceError::Internal(INVALID_MODEL);
        let lm = LearningModel::LoadFromStream(&reference).map_err(rejected)?;
        let session =
            LearningModelSession::CreateFromModelOnDevice(&lm, &device).map_err(rejected)?;

        let eval = |_| internal("ml_inference evaluate");
        let binding = LearningModelBinding::CreateFromSession(&session).map_err(eval)?;
        let x = TensorFloat::CreateFromShapeArrayAndDataArray(&[1, 2], &input).map_err(eval)?;
        binding
            .Bind(
                &HSTRING::from("X"),
                &x.cast::<IInspectable>().map_err(eval)?,
            )
            .map_err(eval)?;
        let result = session.Evaluate(&binding, &HSTRING::new()).map_err(eval)?;
        if !result.Succeeded().map_err(eval)? {
            return Err(internal("ml_inference evaluate"));
        }
        let y = result
            .Outputs()
            .map_err(eval)?
            .Lookup(&HSTRING::from("Y"))
            .map_err(eval)?
            .cast::<TensorFloat>()
            .map_err(eval)?
            .GetAsVectorView()
            .map_err(eval)?;
        if y.Size().map_err(eval)? != 1 {
            return Err(internal("ml_inference output"));
        }
        Ok(f64::from(y.GetAt(0).map_err(eval)?))
    }
}

/// Windows の `MlInference` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn MlInference> {
    Arc::new(WindowsMlInference)
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    const D: Duration = Duration::from_secs(10);

    #[test]
    fn valid_gives_24_or_unsupported() {
        match WindowsMlInference.infer(DemoModel::Valid, [4.0, 5.0], D) {
            Ok(y) => assert_eq!(y, 24.0),
            Err(MlInferenceError::Unsupported) => {}
            Err(e) => panic!("{e:?}"),
        }
    }

    #[test]
    fn corrupt_never_succeeds() {
        assert!(matches!(
            WindowsMlInference.infer(DemoModel::Corrupt, [4.0, 5.0], D),
            Err(MlInferenceError::Internal(INVALID_MODEL)) | Err(MlInferenceError::Unsupported)
        ));
    }
}
