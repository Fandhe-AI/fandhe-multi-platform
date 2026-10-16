//! Android 向け機械学習推論（Issue #292、親 #89 の b-android）。NNAPI（`libneuralnetworks.so`）を
//! `dlopen` で動的に読み、固定の線形モデル `y = 2·x0 + 3·x1 + 1` を `FULLY_CONNECTED` 1 演算で
//! 組んで 1 回推論する。`device/ml_inference.rs` の Android 経路（dispatch は #289 で最終形）から
//! 呼ばれ、契約（結果の種類・期限・守る範囲）の SSOT は `fandhe_device::ml_inference`。
//!
//! - NNAPI は API 27 以上。minSdk 24 のためリンクせず `dlopen` し、読めなければ `Unsupported`。
//!   Android 15 で非推奨だが動作は続く（公式）。LiteRT 2.x は推移依存に GMS が入るため採らない。
//! - 権限・Manifest・Java は不要（JNI を使わない）。
//! - 期限（契約表 3）はドライバだけが持つ。NNAPI の実行は OS 側で中断できず `remaining` は使わない。
//!   期限後に届いた結果はドライバが捨てる。
//! - `Corrupt` は `FULLY_CONNECTED` の入力数を 1 個減らして `addOperation` に渡す。入力数の検証で
//!   `BAD_DATA` になり、実行時ドライバに依存せず確実に拒否される。
//! - `unsafe` は `dlopen` / `dlsym` / NNAPI 呼び出しに限り、公開は安全な `implementation` のみ。
//!   OS のコード・`dlerror` の文字列はログにも画面にも出さない（security.md A09）。
//! - ホストではコンパイルされない（`device.rs` が `target_os = "android"` で宣言）。

// クレートは `deny(unsafe_code)`。FFI 境界のこのモジュールだけ許可し、各 `unsafe` に SAFETY を付ける。
#![allow(unsafe_code)]

use std::ffi::{c_void, CStr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use fandhe_device::ml_inference::{DemoModel, MlInference, BIAS, INVALID_MODEL, WEIGHTS};
use fandhe_device::DeviceError;

// NeuralNetworks.h の定数。
const TENSOR_FLOAT32: i32 = 3;
const INT32: i32 = 1;
const OP_FULLY_CONNECTED: i32 = 9;
const FUSED_NONE: i32 = 0;
const PREFER_FAST_SINGLE_ANSWER: i32 = 1;

const NO_ERROR: i32 = 0;
const OUT_OF_MEMORY: i32 = 1;
const INCOMPLETE: i32 = 2;
const UNEXPECTED_NULL: i32 = 3;
const BAD_DATA: i32 = 4;
const OP_FAILED: i32 = 5;
const BAD_STATE: i32 = 6;
const UNMAPPABLE: i32 = 7;
const OUTPUT_INSUFFICIENT_SIZE: i32 = 8;
const UNAVAILABLE_DEVICE: i32 = 9;

/// `ANeuralNetworksOperandType`。
#[repr(C)]
struct OperandType {
    type_: i32,
    dimension_count: u32,
    dimensions: *const u32,
    scale: f32,
    zero_point: i32,
}

type Ptr = *mut c_void;
type FreeFn = unsafe extern "C" fn(Ptr);

/// NNAPI の関数表。シグネチャは NDK の `NeuralNetworks.h`（API 27）に合わせる。
struct Api {
    model_create: unsafe extern "C" fn(*mut Ptr) -> i32,
    model_free: FreeFn,
    model_add_operand: unsafe extern "C" fn(Ptr, *const OperandType) -> i32,
    model_set_operand_value: unsafe extern "C" fn(Ptr, i32, *const c_void, usize) -> i32,
    model_add_operation: unsafe extern "C" fn(Ptr, i32, u32, *const u32, u32, *const u32) -> i32,
    model_identify_io: unsafe extern "C" fn(Ptr, u32, *const u32, u32, *const u32) -> i32,
    model_finish: unsafe extern "C" fn(Ptr) -> i32,
    comp_create: unsafe extern "C" fn(Ptr, *mut Ptr) -> i32,
    comp_free: FreeFn,
    comp_set_preference: unsafe extern "C" fn(Ptr, i32) -> i32,
    comp_finish: unsafe extern "C" fn(Ptr) -> i32,
    exec_create: unsafe extern "C" fn(Ptr, *mut Ptr) -> i32,
    exec_free: FreeFn,
    exec_set_input: unsafe extern "C" fn(Ptr, i32, *const OperandType, *const c_void, usize) -> i32,
    exec_set_output: unsafe extern "C" fn(Ptr, i32, *const OperandType, *mut c_void, usize) -> i32,
    exec_start_compute: unsafe extern "C" fn(Ptr, *mut Ptr) -> i32,
    event_wait: unsafe extern "C" fn(Ptr) -> i32,
    event_free: FreeFn,
}

/// `dlsym` で関数ポインタを引く。見つからなければ `None`。
///
/// # Safety
/// `T` は呼び出し先のシグネチャと一致する `extern "C" fn` 型であること。
unsafe fn sym<T: Copy>(lib: Ptr, name: &CStr) -> Option<T> {
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<Ptr>());
    // SAFETY: `lib` は `dlopen` が返した有効なハンドル、`name` は NUL 終端。
    let p = unsafe { libc::dlsym(lib, name.as_ptr()) };
    if p.is_null() {
        None
    } else {
        // SAFETY: 関数ポインタとデータポインタは同じ幅（assert 済み）。型の一致は呼び出し側の責任。
        Some(unsafe { std::mem::transmute_copy::<Ptr, T>(&p) })
    }
}

fn load() -> Option<Api> {
    // SAFETY: ライブラリ名は固定リテラル（NUL 終端）。システム名前空間から解決される。
    let lib = unsafe {
        libc::dlopen(
            c"libneuralnetworks.so".as_ptr(),
            libc::RTLD_NOW | libc::RTLD_LOCAL,
        )
    };
    if lib.is_null() {
        return None;
    }
    // ハンドルは dlclose せず保持し続ける（片付けはプロセス終了に任せる。契約表 11）。
    macro_rules! s {
        ($n:literal) => {
            // SAFETY: フィールド型は NeuralNetworks.h のシグネチャと一致させてある。
            unsafe { sym(lib, $n)? }
        };
    }
    Some(Api {
        model_create: s!(c"ANeuralNetworksModel_create"),
        model_free: s!(c"ANeuralNetworksModel_free"),
        model_add_operand: s!(c"ANeuralNetworksModel_addOperand"),
        model_set_operand_value: s!(c"ANeuralNetworksModel_setOperandValue"),
        model_add_operation: s!(c"ANeuralNetworksModel_addOperation"),
        model_identify_io: s!(c"ANeuralNetworksModel_identifyInputsAndOutputs"),
        model_finish: s!(c"ANeuralNetworksModel_finish"),
        comp_create: s!(c"ANeuralNetworksCompilation_create"),
        comp_free: s!(c"ANeuralNetworksCompilation_free"),
        comp_set_preference: s!(c"ANeuralNetworksCompilation_setPreference"),
        comp_finish: s!(c"ANeuralNetworksCompilation_finish"),
        exec_create: s!(c"ANeuralNetworksExecution_create"),
        exec_free: s!(c"ANeuralNetworksExecution_free"),
        exec_set_input: s!(c"ANeuralNetworksExecution_setInput"),
        exec_set_output: s!(c"ANeuralNetworksExecution_setOutput"),
        exec_start_compute: s!(c"ANeuralNetworksExecution_startCompute"),
        event_wait: s!(c"ANeuralNetworksEvent_wait"),
        event_free: s!(c"ANeuralNetworksEvent_free"),
    })
}

static API: OnceLock<Option<Api>> = OnceLock::new();

/// 呼び出し段階。モデル構築での `BAD_DATA` / `BAD_STATE` だけが「モデル拒否」になる。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Build,
    Execute,
}

/// NNAPI の結果コードを契約上のエラーへ写す。元のコードを `-1` に潰さず固定識別子で区別する。
fn map_code(stage: Stage, code: i32) -> DeviceError {
    DeviceError::Internal(match (stage, code) {
        (Stage::Build, BAD_DATA | BAD_STATE) => INVALID_MODEL,
        (_, OUT_OF_MEMORY) => "ml_inference nnapi out_of_memory",
        (_, INCOMPLETE) => "ml_inference nnapi incomplete",
        (_, UNEXPECTED_NULL) => "ml_inference nnapi unexpected_null",
        (_, BAD_DATA) => "ml_inference nnapi bad_data",
        (_, OP_FAILED) => "ml_inference nnapi op_failed",
        (_, BAD_STATE) => "ml_inference nnapi bad_state",
        (_, UNMAPPABLE) => "ml_inference nnapi unmappable",
        (_, OUTPUT_INSUFFICIENT_SIZE) => "ml_inference nnapi output_insufficient_size",
        (_, UNAVAILABLE_DEVICE) => "ml_inference nnapi unavailable_device",
        _ => "ml_inference nnapi unknown",
    })
}

/// `NO_ERROR` だけを先へ進める。
fn check(stage: Stage, code: i32) -> Result<(), DeviceError> {
    if code == NO_ERROR {
        Ok(())
    } else {
        Err(map_code(stage, code))
    }
}

/// NNAPI ハンドルの解放ガード。`?` による早期 return を含む全経路で一度だけ free する（契約表 9）。
struct Handle {
    ptr: Ptr,
    free: FreeFn,
}

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: `ptr` は対応する create / startCompute が返した有効なハンドルで、ここでしか解放しない。
            unsafe { (self.free)(self.ptr) };
        }
    }
}

/// 線形モデルの重みと bias（`fandhe_device::ml_inference` の SSOT から作る）。
fn weights_f32() -> ([f32; 2], [f32; 1]) {
    ([WEIGHTS[0] as f32, WEIGHTS[1] as f32], [BIAS as f32])
}

fn tensor(dims: &[u32]) -> OperandType {
    OperandType {
        type_: TENSOR_FLOAT32,
        dimension_count: dims.len() as u32,
        dimensions: dims.as_ptr(),
        scale: 0.0,
        zero_point: 0,
    }
}

fn run(api: &Api, model_kind: DemoModel, input: [f32; 2]) -> Result<f64, DeviceError> {
    use Stage::{Build, Execute};
    let (weights, bias) = weights_f32();
    let activation: i32 = FUSED_NONE;
    let (d_in, d_w, d_b, d_out) = ([1u32, 2], [1u32, 2], [1u32], [1u32, 1]);
    let (t_in, t_w, t_b, t_out) = (tensor(&d_in), tensor(&d_w), tensor(&d_b), tensor(&d_out));
    let t_act = OperandType {
        type_: INT32,
        dimension_count: 0,
        dimensions: std::ptr::null(),
        scale: 0.0,
        zero_point: 0,
    };

    let mut raw: Ptr = std::ptr::null_mut();
    // SAFETY: `raw` は書き込み可能な出力先。
    check(Build, unsafe { (api.model_create)(&mut raw) })?;
    let model = Handle {
        ptr: raw,
        free: api.model_free,
    };

    for t in [&t_in, &t_w, &t_b, &t_act, &t_out] {
        // SAFETY: model は有効、`t` とその `dimensions` は呼び出しの間生きている。
        check(Build, unsafe { (api.model_add_operand)(model.ptr, t) })?;
    }
    // SAFETY: バッファは 128 バイト以下で即コピーされ、長さは実サイズ。
    unsafe {
        check(
            Build,
            (api.model_set_operand_value)(
                model.ptr,
                1,
                weights.as_ptr().cast(),
                std::mem::size_of_val(&weights),
            ),
        )?;
        check(
            Build,
            (api.model_set_operand_value)(
                model.ptr,
                2,
                bias.as_ptr().cast(),
                std::mem::size_of_val(&bias),
            ),
        )?;
        check(
            Build,
            (api.model_set_operand_value)(model.ptr, 3, (&activation as *const i32).cast(), 4),
        )?;
    }
    // Corrupt は入力数から活性化 operand を抜く。入力数の検証で BAD_DATA になる。
    let op_inputs: &[u32] = match model_kind {
        DemoModel::Valid => &[0, 1, 2, 3],
        DemoModel::Corrupt => &[0, 1, 2],
    };
    let (io_in, io_out) = ([0u32], [4u32]);
    // SAFETY: 配列は呼び出しの間生きており、個数は実長と一致する。
    unsafe {
        check(
            Build,
            (api.model_add_operation)(
                model.ptr,
                OP_FULLY_CONNECTED,
                op_inputs.len() as u32,
                op_inputs.as_ptr(),
                1,
                io_out.as_ptr(),
            ),
        )?;
        check(
            Build,
            (api.model_identify_io)(model.ptr, 1, io_in.as_ptr(), 1, io_out.as_ptr()),
        )?;
        check(Build, (api.model_finish)(model.ptr))?;
    }

    let mut raw: Ptr = std::ptr::null_mut();
    // SAFETY: model は finish 済みで有効。
    check(Build, unsafe { (api.comp_create)(model.ptr, &mut raw) })?;
    let comp = Handle {
        ptr: raw,
        free: api.comp_free,
    };
    // SAFETY: comp は有効。
    unsafe {
        check(
            Build,
            (api.comp_set_preference)(comp.ptr, PREFER_FAST_SINGLE_ANSWER),
        )?;
        check(Build, (api.comp_finish)(comp.ptr))?;
    }

    let mut raw: Ptr = std::ptr::null_mut();
    // SAFETY: comp は finish 済みで有効。
    check(Execute, unsafe { (api.exec_create)(comp.ptr, &mut raw) })?;
    let exec = Handle {
        ptr: raw,
        free: api.exec_free,
    };
    let mut out = [0f32; 1];
    // SAFETY: `input` / `out` は exec の解放まで生きており、長さは実サイズ。
    unsafe {
        check(
            Execute,
            (api.exec_set_input)(
                exec.ptr,
                0,
                std::ptr::null(),
                input.as_ptr().cast(),
                std::mem::size_of_val(&input),
            ),
        )?;
        check(
            Execute,
            (api.exec_set_output)(
                exec.ptr,
                0,
                std::ptr::null(),
                out.as_mut_ptr().cast(),
                std::mem::size_of_val(&out),
            ),
        )?;
    }
    let mut raw: Ptr = std::ptr::null_mut();
    // SAFETY: exec は入出力設定済みで有効。
    check(Execute, unsafe {
        (api.exec_start_compute)(exec.ptr, &mut raw)
    })?;
    let event = Handle {
        ptr: raw,
        free: api.event_free,
    };
    // SAFETY: event は有効。完了（NO_ERROR）を確認してから出力を読む。
    check(Execute, unsafe { (api.event_wait)(event.ptr) })?;
    Ok(f64::from(out[0]))
}

struct NnapiMlInference;

impl MlInference for NnapiMlInference {
    fn infer(
        &self,
        model: DemoModel,
        input: [f32; 2],
        _remaining: Duration,
    ) -> Result<f64, DeviceError> {
        match API.get_or_init(load) {
            Some(api) => run(api, model, input),
            None => Err(DeviceError::Unsupported("ml_inference")),
        }
    }
}

/// Android の `MlInference` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn MlInference> {
    Arc::new(NnapiMlInference)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_bad_data_and_state_are_invalid_model() {
        for c in [BAD_DATA, BAD_STATE] {
            assert!(matches!(
                map_code(Stage::Build, c),
                DeviceError::Internal(INVALID_MODEL)
            ));
        }
    }

    #[test]
    fn execute_codes_are_not_invalid_model_and_keep_identity() {
        let e = map_code(Stage::Execute, BAD_DATA);
        assert!(matches!(e, DeviceError::Internal(s) if s != INVALID_MODEL));
        assert_ne!(
            format!("{:?}", map_code(Stage::Execute, OP_FAILED)),
            format!("{:?}", map_code(Stage::Execute, UNMAPPABLE))
        );
    }

    #[test]
    fn weights_follow_ssot() {
        assert_eq!(weights_f32(), ([2.0, 3.0], [1.0]));
    }
}
