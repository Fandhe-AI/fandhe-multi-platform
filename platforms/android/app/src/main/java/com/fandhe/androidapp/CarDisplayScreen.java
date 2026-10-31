package com.fandhe.androidapp;

import android.os.Handler;
import android.os.Looper;

import androidx.annotation.NonNull;
import androidx.car.app.CarContext;
import androidx.car.app.Screen;
import androidx.car.app.model.Action;
import androidx.car.app.model.PaneTemplate;
import androidx.car.app.model.Pane;
import androidx.car.app.model.Row;
import androidx.car.app.model.Template;
import androidx.lifecycle.DefaultLifecycleObserver;
import androidx.lifecycle.LifecycleOwner;

// 車載ディスプレイ（Issue #1117、親 #152）の画面。`CarDisplayService` の Session が作り、
// ホスト（Android Auto）が `onGetTemplate` の `PaneTemplate` を描く。見出しと行の title は固定で、
// 行の本文だけ共通コアの文言に差し替える（テンプレート数の上限に数えられない、公式）。
// 文言と「現在出ている Screen」は MainActivity ではなくプロセス内 static に置く。Activity が
// 作り直されても失われず、MainActivity が無くても（ホストがこのサービスを起こしても）動く。
// Rust の `android_car_display.rs` が MainActivity 経由で `setText` / `isShowing` を呼ぶ。
// 文言はログ・例外に出さない。切り離し時はこのクラスごと削除する。
final class CarDisplayScreen extends Screen {
    private static final Object LOCK = new Object();
    // null は未設定。
    private static String text;
    // STARTED 以上の Screen。古い Screen の onStop が新しい Screen を消さないよう参照で照合する。
    private static CarDisplayScreen showing;

    // 最後に描いた文言。止まっている間に変わった分を onStart で反映するために持つ。
    private String drawn;

    CarDisplayScreen(@NonNull CarContext carContext) {
        super(carContext);
        getLifecycle().addObserver(new DefaultLifecycleObserver() {
            @Override
            public void onStart(@NonNull LifecycleOwner owner) {
                boolean stale;
                synchronized (LOCK) {
                    showing = CarDisplayScreen.this;
                    stale = !java.util.Objects.equals(drawn, text);
                }
                if (stale) {
                    try {
                        invalidate();
                    } catch (RuntimeException ignored) {
                        // HostException 等。observer の外へ投げると同じプロセスの MainActivity ごと落ちる。
                        // 文言は保存済みで、次の onGetTemplate で反映される。
                    }
                }
            }

            @Override
            public void onStop(@NonNull LifecycleOwner owner) {
                synchronized (LOCK) {
                    if (showing == CarDisplayScreen.this) {
                        showing = null;
                    }
                }
            }
        });
    }

    // 文言を置き、車載画面が出ていれば main で再描画を頼む。出ていなくても成功（受け付けまで）。
    static void setText(String t) {
        synchronized (LOCK) {
            text = t;
        }
        // post が false でも文言は保存済みで、次の onGetTemplate / onStart で反映される。
        new Handler(Looper.getMainLooper()).post(() -> {
            try {
                CarDisplayScreen s;
                synchronized (LOCK) {
                    s = showing;
                }
                if (s != null) {
                    s.invalidate();
                }
            } catch (RuntimeException ignored) {
                // 再描画の失敗は握る（契約）。
            }
        });
    }

    static boolean isShowing() {
        synchronized (LOCK) {
            return showing != null;
        }
    }

    @NonNull
    @Override
    public Template onGetTemplate() {
        String t;
        synchronized (LOCK) {
            t = text;
            drawn = t;
        }
        if (t == null) {
            t = getCarContext().getString(R.string.car_display_unset);
        }
        Row row = new Row.Builder()
                .setTitle(getCarContext().getString(R.string.car_display_row_title))
                .addText(t)
                .build();
        return new PaneTemplate.Builder(new Pane.Builder().addRow(row).build())
                .setTitle(getCarContext().getString(R.string.car_display_title))
                .setHeaderAction(Action.APP_ICON)
                .build();
    }
}
