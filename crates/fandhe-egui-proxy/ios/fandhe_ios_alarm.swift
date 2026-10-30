// アラーム・正確な時刻の予約（AlarmKit）の Swift 薄層（Issue #1077、親 #1056）。
//
// `AlarmManager` は Swift 専用 API で ObjC から呼べないため、`build.rs` の `compile_ios_alarm` が
// swiftc で静的ライブラリ化し、Rust の `device/ios_alarm.rs` が `@_cdecl` の C ABI 3 関数だけを呼ぶ。
// AlarmKit は iOS 26 以降の API のため弱リンクし、`canImport` と `#available` で囲む。
// 26 専用 API は旧 SDK（Xcode 16 は Swift 6.1 以下）に無く `#available` では型検査を除外できないため、
// `compiler(>=6.2)`（Xcode 26 以降）でも囲み、旧 SDK では 1（非対応）にする。
//
// 契約（戻りコードの意味の SSOT は Rust 側の `match`。ここは同じ表の写し）:
//   0 成功（state では未予約）/ 1 非対応（iOS 26 未満・旧 SDK）/ 2 許可なし /
//   3 予約数の上限 / 4 予約の失敗 / 5 予約後に読み直しても同 id が無い /
//   6 一覧の読み取り失敗（.fixed でない予約・時刻が不正な値を含む）/
//   7 取り消しの失敗 / 8 予約中（state 専用。out_ms に鳴る時刻を書く）
//
// - 期限は掛けない。`schedule`・`requestAuthorization` は中断できないため、完了を待って実結果を返す
//   （期限切れで戻った後に OS の状態が変わる嘘の失敗を作らない）。
//   ponytail: OS が戻らなければ処理中のまま。実機で詰まる例が出たら「期限 + 戻った後は一覧で確かめ直す」に上げる。
// - 予約は固定 UUID 1 件だけ。二重予約は古い予約を消してから足す。消せなければ新しい予約は作らない。
// - 予約後に一覧を読み直し、同 id が無ければ 5（許可 UI を出しただけ・黙って捨てられた場合を成功にしない）。
// - Widget Extension・Live Activity は足さない（固定時刻の一回きりのアラームで、カウントダウン表示を持たない）。
// - ポインタは呼び出しの間だけ使い、Task へは持ち込まない。main queue・`dispatch_sync` は使わない
//   （呼び出し元は専用ワーカー）。エラーの内容はログへ出さない。

import Foundation
#if canImport(AlarmKit) && compiler(>=6.2)
import AlarmKit
import SwiftUI
#endif

/// 非同期 API の結果を 1 回だけ受け取る箱。Task とワーカーが共有するのはこれだけ。
private final class ResultBox<T: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: T?

    func set(_ v: T) {
        lock.lock()
        defer { lock.unlock() }
        if value == nil { value = v }
    }

    func take() -> T? {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
}

#if canImport(AlarmKit) && compiler(>=6.2)
/// 本アプリの予約を識別する固定 id（予約は常に高々 1 件）。
private let alarmId = UUID(uuidString: "5C1D6E52-3A0B-4D54-9F51-7B2A8E0C41F7")!

/// `AlarmAttributes` が要求する型引数。中身は持たない。
@available(iOS 26.0, *)
private struct FandheAlarmMetadata: AlarmMetadata {}

@available(iOS 26.0, *)
private func presentation() -> AlarmPresentation {
    let title: LocalizedStringResource = "Fandhe のアラーム"
    // stopButton 付きは 26.0 / 26.1 以降の両 SDK で型検査が通る（省略版は 26.1 SDK のみ）。
    let stop = AlarmButton(text: "停止", textColor: .white, systemImageName: "stop.circle")
    return AlarmPresentation(alert: .init(title: title, stopButton: stop))
}

/// async API を同期に畳む。期限は掛けない（中断できないため）。
@available(iOS 26.0, *)
private func blocking<T: Sendable>(_ body: @escaping @Sendable () async -> T) -> T {
    let box = ResultBox<T>()
    let sem = DispatchSemaphore(value: 0)
    Task.detached {
        box.set(await body())
        sem.signal()
    }
    sem.wait()
    return box.take()!
}

/// 許可を確かめ、未決定ならダイアログの応答を待つ。0 = 許可済み。
@available(iOS 26.0, *)
private func ensureAuthorized() -> Int32 {
    let manager = AlarmManager.shared
    switch manager.authorizationState {
    case .authorized:
        return 0
    case .denied:
        return 2
    case .notDetermined:
        let r: Int32 = blocking {
            do {
                let s = try await AlarmManager.shared.requestAuthorization()
                return s == .authorized ? 0 : 2
            } catch {
                return 4
            }
        }
        return r
    @unknown default:
        return 4
    }
}

@available(iOS 26.0, *)
private func hasOurAlarm() -> Bool? {
    guard let list = try? AlarmManager.shared.alarms else { return nil }
    return list.contains { $0.id == alarmId }
}

@available(iOS 26.0, *)
private func scheduleImpl(_ fireAtMs: UInt64) -> Int32 {
    let auth = ensureAuthorized()
    if auth != 0 { return auth }
    let manager = AlarmManager.shared
    guard let present = hasOurAlarm() else { return 6 }
    if present {
        // 古い予約を消せなければ新しい予約を作らない。
        do { try manager.cancel(id: alarmId) } catch { return 7 }
    }
    let date = Date(timeIntervalSince1970: Double(fireAtMs) / 1000.0)
    let attributes = AlarmAttributes<FandheAlarmMetadata>(
        presentation: presentation(), metadata: nil, tintColor: .orange)
    let config = AlarmManager.AlarmConfiguration<FandheAlarmMetadata>.alarm(
        schedule: .fixed(date), attributes: attributes)
    let r: Int32 = blocking {
        do {
            _ = try await AlarmManager.shared.schedule(id: alarmId, configuration: config)
            return 0
        } catch let e as AlarmManager.AlarmError {
            return e == .maximumLimitReached ? 3 : 4
        } catch {
            return 4
        }
    }
    if r != 0 { return r }
    guard let now = hasOurAlarm() else { return 6 }
    return now ? 0 : 5
}

@available(iOS 26.0, *)
private func cancelImpl() -> Int32 {
    guard let present = hasOurAlarm() else { return 6 }
    if !present { return 0 }
    do {
        try AlarmManager.shared.cancel(id: alarmId)
        return 0
    } catch {
        // throw でも消えていることがあるので、一覧で確かめてから成否を決める。
        return hasOurAlarm() == false ? 0 : 7
    }
}

@available(iOS 26.0, *)
private func stateImpl(_ out: UnsafeMutablePointer<UInt64>) -> Int32 {
    guard let list = try? AlarmManager.shared.alarms else { return 6 }
    guard let mine = list.first(where: { $0.id == alarmId }) else { return 0 }
    // alerting 中も一覧に残るので予約中として扱う（trait doc どおり）。
    guard case .fixed(let d)? = mine.schedule else { return 6 }
    let ms = (d.timeIntervalSince1970 * 1000).rounded()
    guard ms.isFinite, ms >= 0, ms < Double(UInt64.max) else { return 6 }
    out.pointee = UInt64(ms)
    return 8
}
#endif

@_cdecl("fandhe_ios_alarm_schedule")
public func fandhe_ios_alarm_schedule(_ fireAtMs: UInt64) -> Int32 {
    #if canImport(AlarmKit) && compiler(>=6.2)
    guard #available(iOS 26.0, *) else { return 1 }
    return scheduleImpl(fireAtMs)
    #else
    return 1
    #endif
}

@_cdecl("fandhe_ios_alarm_cancel")
public func fandhe_ios_alarm_cancel() -> Int32 {
    #if canImport(AlarmKit) && compiler(>=6.2)
    guard #available(iOS 26.0, *) else { return 1 }
    return cancelImpl()
    #else
    return 1
    #endif
}

@_cdecl("fandhe_ios_alarm_state")
public func fandhe_ios_alarm_state(_ outMs: UnsafeMutablePointer<UInt64>?) -> Int32 {
    guard let outMs = outMs else { return 4 }
    #if canImport(AlarmKit) && compiler(>=6.2)
    guard #available(iOS 26.0, *) else { return 1 }
    return stateImpl(outMs)
    #else
    return 1
    #endif
}
