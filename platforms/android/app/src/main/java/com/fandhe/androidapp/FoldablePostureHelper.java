package com.fandhe.androidapp;

import android.app.Activity;
import android.graphics.Rect;

import androidx.core.content.ContextCompat;
import androidx.core.util.Consumer;
import androidx.window.java.layout.WindowInfoTrackerCallbackAdapter;
import androidx.window.layout.DisplayFeature;
import androidx.window.layout.FoldingFeature;
import androidx.window.layout.WindowInfoTracker;
import androidx.window.layout.WindowLayoutInfo;
import androidx.window.layout.WindowMetricsCalculator;

import java.util.ArrayList;
import java.util.List;

// 折りたたみの姿勢・ヒンジ（Issue #1098、親 #141）の Android 実装ヘルパー。MainActivity から
// 生成・委譲され、Rust の `device/android_foldable_posture.rs` が egui スレッドから
// `MainActivity.foldablePostureRead` を JNI で 1 秒間隔に呼ぶ（pull 型。Java から Rust への push は
// 作らない）。値の解釈と検査は Rust 側 `decode` が行い、ここでは生の整数のまま渡す。
//
// 購読: `onStart` で `WindowInfoTrackerCallbackAdapter.addWindowLayoutInfoListener` を main executor
// で登録し、`onStop` で解除してキャッシュを捨てる。通知のたびに、その時点の窓の寸法・密度と
// `FoldingFeature`（`DisplayFeature` のうち折れ目だけ）を int 配列へ書く。bounds は窓座標。
//
// スナップショット `int[]`（SSOT はここ。Rust 側 `android_foldable_posture.rs` が同値で読む）:
//   [0] status: STATUS_NONE（未受信）/ STATUS_READY（受信済み）/ STATUS_FAILED（失敗）
//   STATUS_READY のみ: [1] 窓の幅 px, [2] 窓の高さ px, [3] densityDpi, [4] 件数 n,
//   [5 + 6i ..] state, orientation, left, top, right, bottom（i 番目の折れ目）
//
// ライフサイクル: 全状態を LOCK の内側で読み書きする（通知は main、読み取りは egui スレッド）。
// 登録のたびに epoch を進め、自分の epoch を捕まえた consumer をフィールドへ保存してから add する
// ため、登録から参照の保存までに届いた通知も落とさない。`onStop` / `onDestroy` は epoch を進めて
// main queue に残った古い通知を無効にし、未受信へ戻す。解除に失敗したときはリスナーの参照を残して
// STATUS_FAILED を返し、次の `onStart` で解除をやり直す（握りつぶして閉じた扱いにしない）。
// 再生成では旧インスタンスが破棄とキャッシュ消去を済ませ、新インスタンスは未受信から始まる。
// 値（機種を推定できる寸法）と例外の内容はログに出さない。
//
// 守る範囲: 未受信と失敗を「平ら」にしないこと・`onStop` 後に前回値を返さないこと・購読の登録と
// 解除の対応。守らない範囲: 反映の遅れ（1 秒 + 1 フレーム）、再生成直後の一時的な未受信、
// 閉じた端末と広げた端末の区別、ヒンジの角度。
final class FoldablePostureHelper {
    static final int STATUS_NONE = 0;
    static final int STATUS_READY = 1;
    static final int STATUS_FAILED = 2;

    // FoldingFeature の state / orientation の写像。-1 は未知で、Rust 側が Internal にする。
    static final int STATE_FLAT = 0;
    static final int STATE_HALF_OPENED = 1;
    static final int ORIENTATION_VERTICAL = 0;
    static final int ORIENTATION_HORIZONTAL = 1;
    static final int UNKNOWN = -1;

    private final Activity activity;
    private final Object LOCK = new Object();
    // 以下は LOCK の内側でのみ読み書きする。
    private int[] snapshot = new int[] {STATUS_NONE};
    private WindowInfoTrackerCallbackAdapter adapter;
    private Consumer<WindowLayoutInfo> listener;
    private boolean unreleased;
    private boolean destroyed;
    private long epoch;

    FoldablePostureHelper(Activity activity) {
        this.activity = activity;
    }

    void onStart() {
        synchronized (LOCK) {
            if (destroyed) {
                return;
            }
            if (listener != null && !release()) {
                // 前回の解除が残っている。新しく登録せず失敗のままにする。
                return;
            }
            final long mine = ++epoch;
            final Consumer<WindowLayoutInfo> consumer = info -> onLayoutInfo(mine, info);
            WindowInfoTrackerCallbackAdapter a = null;
            try {
                a = new WindowInfoTrackerCallbackAdapter(WindowInfoTracker.getOrCreate(activity));
                adapter = a;
                listener = consumer;
                snapshot = new int[] {STATUS_NONE};
                a.addWindowLayoutInfoListener(activity, ContextCompat.getMainExecutor(activity), consumer);
            } catch (RuntimeException e) {
                adapter = null;
                listener = null;
                snapshot = new int[] {STATUS_FAILED};
            }
        }
    }

    void onStop() {
        synchronized (LOCK) {
            stopLocked();
        }
    }

    void onDestroy() {
        synchronized (LOCK) {
            destroyed = true;
            stopLocked();
        }
    }

    // JNI 窓口。スナップショットの写しを返す（null は返さない）。
    int[] read() {
        synchronized (LOCK) {
            return unreleased ? new int[] {STATUS_FAILED} : snapshot.clone();
        }
    }

    // LOCK を持って呼ぶ。古い通知を無効にして未受信へ戻し、解除する。
    private void stopLocked() {
        epoch++;
        snapshot = new int[] {STATUS_NONE};
        if (listener != null) {
            release();
        }
    }

    // LOCK を持って呼ぶ。成功したら true。失敗時は参照を残して unreleased にする。
    private boolean release() {
        try {
            adapter.removeWindowLayoutInfoListener(listener);
            adapter = null;
            listener = null;
            unreleased = false;
            return true;
        } catch (RuntimeException e) {
            unreleased = true;
            return false;
        }
    }

    // main executor で届く通知。古い世代・破棄後は何もしない。
    private void onLayoutInfo(long mine, WindowLayoutInfo info) {
        synchronized (LOCK) {
            if (mine != epoch || destroyed) {
                return;
            }
            try {
                snapshot = build(info);
            } catch (RuntimeException e) {
                snapshot = new int[] {STATUS_FAILED};
            }
        }
    }

    private int[] build(WindowLayoutInfo info) {
        Rect window = WindowMetricsCalculator.getOrCreate().computeCurrentWindowMetrics(activity).getBounds();
        int dpi = activity.getResources().getConfiguration().densityDpi;
        List<FoldingFeature> folds = new ArrayList<>();
        for (DisplayFeature f : info.getDisplayFeatures()) {
            if (f instanceof FoldingFeature) {
                folds.add((FoldingFeature) f);
            }
        }
        int[] out = new int[5 + 6 * folds.size()];
        out[0] = STATUS_READY;
        out[1] = window.width();
        out[2] = window.height();
        out[3] = dpi;
        out[4] = folds.size();
        int i = 5;
        for (FoldingFeature f : folds) {
            FoldingFeature.State s = f.getState();
            out[i++] = s == FoldingFeature.State.FLAT ? STATE_FLAT
                    : s == FoldingFeature.State.HALF_OPENED ? STATE_HALF_OPENED : UNKNOWN;
            FoldingFeature.Orientation o = f.getOrientation();
            out[i++] = o == FoldingFeature.Orientation.VERTICAL ? ORIENTATION_VERTICAL
                    : o == FoldingFeature.Orientation.HORIZONTAL ? ORIENTATION_HORIZONTAL : UNKNOWN;
            Rect b = f.getBounds();
            out[i++] = b.left;
            out[i++] = b.top;
            out[i++] = b.right;
            out[i++] = b.bottom;
        }
        return out;
    }
}
