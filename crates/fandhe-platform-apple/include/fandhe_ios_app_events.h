// アプリ単位の出来事（URL・activity・背景転送の完了）を機能 crate の `.m` へ配る口の宣言
// （Issue #1325、親 #1159）。署名と通知名の正本で、受け手は手書きの `extern` や文字列リテラルで複製せず
// 本ヘッダを `#import` する。実装は `ios/fandhe_ios_scene.m`（URL・activity の通知）と
// `ios/fandhe_ios_app_delegate.m`（旧経路の URL・背景転送）。共通層は機能の記号を一切参照しない
// （選ばなかった機能の受け口を成果物に残さないため）。
//
// 受け手は URL・identifier をログへ出さないこと。URL は他アプリが送れる入力なので、検査は受け手が行う。

#import <Foundation/Foundation.h>

// URL（カスタム scheme・ファイル・ショートカットの type を URL 化したもの）。object = NSURL。
// main thread から投稿される。scene 形態では scene delegate、scene 形態でない場合は
// `application:openURL:options:`（共通層の app delegate）が投げる。
extern NSString *const FandheSceneOpenURLNotification;

// 継続された user activity（検索結果からの継続等）。object = NSUserActivity。main thread から投稿される。
extern NSString *const FandheSceneContinueUserActivityNotification;

// 共通層の application delegate を差し込む。main thread から `UIApplicationMain` より前に呼ぶ。冪等。
// `UIApplicationDidFinishLaunchingNotification` の時点で `delegate == nil` のときだけ差し込み、UIKit の
// 既定動作は変えない。`fandhe_ios_register_background_session` が内包するため、旧経路の URL だけが要る
// 機能が単独で呼ぶ。
void fandhe_ios_app_delegate_install(void);

// 背景 URLSession の完了通知の受け手。OS が転送完了でアプリを起こしたとき main thread から呼ばれる。
// 受け手は `completionHandler` をちょうど 1 回だけ呼ぶ責任を負う。
typedef void (^FandheBackgroundSessionReceiver)(void (^completionHandler)(void));

// 背景 URLSession の完了を identifier の完全一致で受け取る登録。main thread から `UIApplicationMain` より
// 前に呼ぶ。内部で `fandhe_ios_app_delegate_install` も呼ぶ。同じ identifier の再登録は置き換える。
// 登録の無い identifier の完了は共通層が `completionHandler` を即呼ぶ（成功に見せる処理はしない）。
void fandhe_ios_register_background_session(NSString *identifier,
                                            FandheBackgroundSessionReceiver receiver);
