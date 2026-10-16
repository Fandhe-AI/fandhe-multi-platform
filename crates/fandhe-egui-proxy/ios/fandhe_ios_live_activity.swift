// iOS のライブアクティビティ（ActivityKit）の薄層（Issue #295、親 #88 の b-ios）。
// `src/device/ios_live_activity.rs` から呼ばれる。`Activity<Attributes>` は Swift 専用 API で ObjC から
// 呼べないため、`@_cdecl` で C ABI を公開する（`build.rs` が swiftc で静的ライブラリ化してリンクする）。
//
// # ABI
//
// - `fandhe_ios_live_activity_submit(request_id, op, value, done) -> Int32`
//   op: 0 開始 / 1 更新 / 2 終了。戻り値 0 = 受理（完了は `done` からちょうど 1 回）、0 以外 = 未受理。
//   `done(request_id, op, code)` は main actor 上で呼ばれる。ポインタは渡さず整数と C 関数ポインタのみ。
// - `fandhe_ios_live_activity_dismiss_all()`: 残っている表示を全部閉じる（冪等）。
//
// # 結果コード（写像の SSOT は Rust 側 `outcome_from_code`。ここは分類して送るだけ）
//
// 0 開始を受理 / 1 更新を受理 / 2 終了を受理 / 3 既に閉じられていた / 10 許可なし / 11 非対応（拡張の
// 未同梱を含む）/ 20 その他の ActivityAuthorizationError / 21 未知のエラー・差し替わり / 22 状態が
// pending・未知 / 23 不明な op。
//
// # 並行性
//
// 開始・更新・終了・dismiss_all は main actor 上の直列チェーン（`tail`）に積む。初期化時の片付けが
// 直後の開始で作った Activity を閉じる競合を防ぐ（契約表 2・11）。エラーの内容はログに出さない。
import ActivityKit
import Foundation

public typealias FandheLiveActivityDone = @convention(c) (UInt64, Int32, Int32) -> Void

@MainActor
private enum Chain {
    static var tail: Task<Void, Never>?
    static var current: Activity<FandheLiveActivityAttributes>?

    /// 前の Task の完了を待ってから `work` を実行する。
    static func enqueue(_ work: @escaping @MainActor () async -> Void) {
        let prev = tail
        tail = Task { @MainActor in
            await prev?.value
            await work()
        }
    }
}

/// 拡張（`FandheLiveActivity.appex`）がバンドルに同梱されているか。無ければ表示できない。
private func extensionBundled() -> Bool {
    guard let plugIns = Bundle.main.builtInPlugInsURL else { return false }
    return FileManager.default.fileExists(
        atPath: plugIns.appendingPathComponent("FandheLiveActivity.appex").path)
}

private func code(for error: Error) -> Int32 {
    guard let auth = error as? ActivityAuthorizationError else { return 21 }
    switch auth {
    case .denied: return 10
    case .unsupported, .unsupportedTarget: return 11
    case .attributesTooLarge, .globalMaximumExceeded, .targetMaximumExceeded,
        .malformedActivityIdentifier, .missingProcessIdentifier, .persistenceFailure,
        .reconnectNotPermitted, .unentitled, .visibility:
        return 20
    @unknown default: return 21
    }
}

@MainActor
private func endAll() async {
    for activity in Activity<FandheLiveActivityAttributes>.activities {
        await activity.end(nil, dismissalPolicy: .immediate)
    }
    Chain.current = nil
}

@MainActor
private func run(op: Int32, value: UInt32) async -> Int32 {
    switch op {
    case 0:
        guard extensionBundled() else { return 11 }
        guard ActivityAuthorizationInfo().areActivitiesEnabled else { return 10 }
        await endAll()
        do {
            let content = ActivityContent(
                state: FandheLiveActivityAttributes.ContentState(value: value), staleDate: nil)
            Chain.current = try Activity.request(
                attributes: FandheLiveActivityAttributes(), content: content, pushType: nil)
            return 0
        } catch {
            return code(for: error)
        }
    case 1:
        guard let activity = Chain.current else { return 3 }
        switch activity.activityState {
        case .ended, .dismissed:
            Chain.current = nil
            return 3
        case .active, .stale:
            await activity.update(
                ActivityContent(
                    state: FandheLiveActivityAttributes.ContentState(value: value), staleDate: nil))
            // await の間に差し替わっていたら成功にしない。
            return Chain.current?.id == activity.id ? 1 : 21
        default:
            return 22
        }
    case 2:
        guard let activity = Chain.current else { return 2 }
        await activity.end(nil, dismissalPolicy: .immediate)
        if Chain.current?.id == activity.id { Chain.current = nil }
        return 2
    default:
        return 23
    }
}

@_cdecl("fandhe_ios_live_activity_submit")
public func fandhe_ios_live_activity_submit(
    _ requestId: UInt64, _ op: Int32, _ value: UInt32, _ done: FandheLiveActivityDone
) -> Int32 {
    Task { @MainActor in
        Chain.enqueue {
            let result = await run(op: op, value: value)
            done(requestId, op, result)
        }
    }
    return 0
}

@_cdecl("fandhe_ios_live_activity_dismiss_all")
public func fandhe_ios_live_activity_dismiss_all() {
    Task { @MainActor in
        Chain.enqueue { await endAll() }
    }
}
