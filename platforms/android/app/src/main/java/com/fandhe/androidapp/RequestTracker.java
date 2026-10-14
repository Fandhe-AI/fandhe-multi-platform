package com.fandhe.androidapp;

import java.util.concurrent.atomic.AtomicLong;

// MainActivity の各端末機能（共有・ファイル選択・生体認証・読み上げ・カメラ）が
// 「いま待っている要求 id」を記録・照合するための共通クラス（Issue #944）。
//
// MainActivity では機能ごとに手書きの read-then-write（「読んで消す」「一致したら
// 消す」）が別々に実装されており、同種の指摘が機能の数だけ出ていた。本クラスは
// この id の保持・照合だけを 1 箇所へ集約する。復元戦略（static で Activity 再生成を
// 跨いで生存させるか、Bundle へ永続化するか、単に破棄するか）は機能ごとの設計判断
// （Issue #881）のため統一せず、呼び出し元（MainActivity）が引き続き選ぶ
// （static フィールドに載せるか instance フィールドに載せるかも呼び出し元の判断）。
//
// 番兵に 0 ではなく -1（`NONE`）を使う理由: id は Rust 側が採番して JNI 引数で渡す
// （このクラスは発行しない、記録・照合のみ）。共有・ファイル選択・生体認証の
// Rust 側 `NEXT_REQUEST_ID` は 1 始まりのため 0 を番兵にできるが、カメラ
// （`CaptureMailbox`）と読み上げ（`NEXT_TTS_GENERATION`）は 0 始まりで 0 が正当な
// id になり得る。共通番兵を 0 にはできないため -1 を採用する（id は u64 由来で
// 非負のため衝突しない。読み上げの旧 `NO_SPEAKING_REQUEST` と同じ値）。
final class RequestTracker {
    /** 「追跡中の要求なし」を表す番兵（上記 doc 参照）。 */
    static final long NONE = -1L;

    private final AtomicLong current = new AtomicLong(NONE);

    /** JNI で受けた id を「いま待っている要求」として記録する。Bundle からの復元にも使う。 */
    void track(long requestId) {
        current.set(requestId);
    }

    /** 照合せずに現在値を読む（Bundle 保存・「追跡中か」判定・照合前のスナップショット用）。 */
    long current() {
        return current.get();
    }

    /** 読み取りとクリアを 1 操作で行う（first-wins。二重コールバック・プロセス死後の再配送を弾く）。 */
    long take() {
        return current.getAndSet(NONE);
    }

    /** この id がいま待っている要求なら消して true。旧要求の遅延到着なら false（no-op）。 */
    boolean complete(long requestId) {
        return current.compareAndSet(requestId, NONE);
    }
}
