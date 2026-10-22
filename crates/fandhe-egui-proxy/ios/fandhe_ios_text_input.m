// Issue #795: iOS の日本語ソフトキーボード（変換・確定）を egui へ届けるための
// ObjC 側実装。
//
// winit 0.30.13 の iOS ビュー（クラス名 `WinitUIView`、`platform_impl/ios/view.rs`）は
// `UIKeyInput` のみ実装し `UITextInput`（marked text）を持たないため、日本語キーボードの
// 変換中文字列が `insertText:` に一切来ない（英字は届く。#795 計画の実機検証記録参照）。
// 本ファイルは、ゼロサイズの `UITextField`（UIKit が `UITextInput` 全体を実装済み）を
// 代わりに first responder にすることでこれを回避する（SDL などが使う既知の手法）。
//
// C ABI 契約（`crates/fandhe-egui-proxy/src/ios_text_input.rs` 側の呼び出し元と対）:
//   - すべて UIKit のメインスレッド（UI イベントディスパッチ元）から呼ばれる。
//     Rust 側はこれらのコールバックの中で重い処理をしない契約とする。
//   - `fandhe_ios_text_input_changed(text_utf8, compose_start, compose_end)`:
//     `text_utf8` は NUL 終端 UTF-8 文字列への非所有ポインタ（呼び出し中のみ有効、
//     Rust 側はコピーしてから返ること）。`compose_start`/`compose_end` は
//     `text_utf8` を UTF-16 コード単位で数えたオフセット。未確定領域が無ければ
//     両方 -1。
//   - `fandhe_ios_text_input_backspace()` / `_return()`: 引数なし。
//   - `fandhe_ios_text_input_reset()`: フォーカスが新しいフィールドへ移った直後に
//     呼ばれ、Rust 側の直前状態バッファをクリアする契約の通知。
//   - `fandhe_ios_text_input_install()`: 冪等。`WinitUIView` クラスへ動的にメソッドを
//     差し込む（`objc_getClass` で見つからなければ何もしない）。複数回呼んでも
//     2 回目以降は上記差し込みを再実行しない。
//   - `fandhe_ios_text_input_clear()`: egui 側で focus が別ウィジェットへ直接移った
//     （A → B、`resignFirstResponder`/`becomeFirstResponder` の往復を経ない）場合に
//     Rust 側から呼ぶ。フィールド未生成なら no-op。`text` をプログラム的に空へ戻す
//     だけで `UIControlEventEditingChanged` は発火しない（UIKit の仕様、ユーザー操作・
//     `sendActions:for:` 明示呼び出し以外ではこのイベントは飛ばない）ため、
//     `fandhe_ios_text_input_changed` は呼ばれない。
//   - `int32_t fandhe_ios_text_input_set_one_time_code_hint(int32_t on)`（Issue #586、
//     autofill-otp。呼び出し元は `src/device/ios_autofill_otp.rs`）: `on != 0` で入力欄に
//     `textContentType = oneTimeCode` と数字キーボードを設定し、`on == 0` で `nil` と
//     `UIKeyboardTypeDefault`（生成直後の既定値）へ戻す。希望状態はファイル static に保持し、
//     入力欄の生成時にも反映する（未生成でも成功）。first responder なら `reloadInputViews` で
//     キーボードを差し替える。冪等。戻りコード: 0 = 成功、2 = main スレッド以外
//     （UIKit に触れず `dispatch_sync` もしない）、3 = `on != 0` なのに入力欄の橋渡し
//     （`install`）が未完でヒントが効かない（成功を偽らず、希望状態も変えない）。守らない範囲: ヒントは
//     プロセス内で残り、OTP 画面を離れても自動では戻らない。コードの値はこの経路を通らず、
//     ログにも出さない。

#import <UIKit/UIKit.h>
#import <objc/runtime.h>

// Rust 側（`ios_text_input.rs`）が定義するコールバック。
extern void fandhe_ios_text_input_changed(const char *_Nullable text_utf8, long compose_start,
                                           long compose_end);
extern void fandhe_ios_text_input_backspace(void);
extern void fandhe_ios_text_input_return(void);
extern void fandhe_ios_text_input_reset(void);

// ワンタイムコード用ヒントの希望状態と、橋渡しの差し込み済みフラグ。
// いずれも main スレッドでのみ読み書きする。
static BOOL g_oneTimeCodeHint = NO;
static BOOL g_installed = NO;

// 希望状態を入力欄へ反映する。`NO` のときは生成直後の既定値を入れ直すだけなので、
// OTP を使わない日本語入力画面の挙動は変わらない。
static void FandheApplyContentHint(UITextField *field) {
    if (g_oneTimeCodeHint) {
        field.textContentType = UITextContentTypeOneTimeCode;
        field.keyboardType = UIKeyboardTypeNumberPad;
    } else {
        field.textContentType = nil;
        field.keyboardType = UIKeyboardTypeDefault;
    }
}

// ゼロサイズの `UITextField`。UIKit の `UITextInput`（marked text 含む）実装一式を
// そのまま first responder として使い、変換中文字列を `markedTextRange` から読む。
@interface FandheTextField : UITextField <UITextFieldDelegate>
@end

@implementation FandheTextField

- (instancetype)init {
    self = [super initWithFrame:CGRectZero];
    if (self) {
        // 変換候補（かな漢字変換等）を機能させるため autocorrection は既定のまま
        // 有効にする（#795 計画）。autocapitalization・スマート引用符/ダッシュは
        // ソフトキーボード入力の想定外挙動を避けるため無効化する。
        self.autocapitalizationType = UITextAutocapitalizationTypeNone;
        self.autocorrectionType = UITextAutocorrectionTypeDefault;
        self.smartQuotesType = UITextSmartQuotesTypeNo;
        self.smartDashesType = UITextSmartDashesTypeNo;
        self.smartInsertDeleteType = UITextSmartInsertDeleteTypeNo;
        self.spellCheckingType = UITextSpellCheckingTypeNo;
        // hidden だと `becomeFirstResponder` がソフトキーボードを出さない
        // （UIKit の仕様）ため、可視のまま透明にしてレイアウトへ影響させない。
        self.hidden = NO;
        self.alpha = 0;
        self.delegate = self;
        FandheApplyContentHint(self);
        [self addTarget:self
                      action:@selector(fandheTextChanged)
            forControlEvents:UIControlEventEditingChanged];
    }
    return self;
}

// `deleteBackward` は `UITextInput` プロトコルの一部。`text` が既に空のときは
// UIKit 側で「消せる文字が無い」ため何も起きず `UIControlEventEditingChanged` も
// 発火しない。その場合のみ Rust 側へ直接 Backspace を通知する
// （#795 計画: 既存テキストが無い状態で Backspace キーが押されたことを伝える経路）。
- (void)deleteBackward {
    if (self.text.length == 0) {
        fandhe_ios_text_input_backspace();
        return;
    }
    [super deleteBackward];
}

- (void)fandheTextChanged {
    NSString *text = self.text ?: @"";
    long composeStart = -1;
    long composeEnd = -1;
    UITextRange *marked = self.markedTextRange;
    if (marked != nil) {
        composeStart = [self offsetFromPosition:self.beginningOfDocument toPosition:marked.start];
        composeEnd = [self offsetFromPosition:self.beginningOfDocument toPosition:marked.end];
    }
    fandhe_ios_text_input_changed(text.UTF8String, composeStart, composeEnd);
}

- (BOOL)textFieldShouldReturn:(UITextField *)textField {
    fandhe_ios_text_input_return();
    return NO;
}

@end

// `WinitUIView`（`objc_getClass` で実行時に解決、`winit` 側の宣言に直接依存しない）の
// 差し込み対象。`FandheTextField` は遅延生成し、生成済みインスタンスを紐付けて
// 使い回す（`objc_setAssociatedObject`）。
static const void *kFandheTextFieldKey = &kFandheTextFieldKey;

// 実質単一ウィンドウ前提（`WinitUIView` は 1 アプリにつき通常 1 インスタンス）で、
// `fandhe_ios_text_input_clear` から view を経由せず直接参照するための直近生成分の
// 保持（ponytail: 複数ウィンドウ対応が要件化したら `objc_getAssociatedObject` 経由に
// 揃える）。
static FandheTextField *_Nullable g_activeField = nil;

static FandheTextField *_Nonnull FandheEnsureTextField(UIView *view) {
    FandheTextField *field = objc_getAssociatedObject(view, kFandheTextFieldKey);
    if (field == nil) {
        field = [[FandheTextField alloc] init];
        [view addSubview:field];
        objc_setAssociatedObject(view, kFandheTextFieldKey, field, OBJC_ASSOCIATION_RETAIN);
    }
    g_activeField = field;
    return field;
}

// `class_replaceMethod` で差し込む `becomeFirstResponder` / `resignFirstResponder` の
// 実装。`self` は `WinitUIView` のインスタンス（`UIView` として扱う）。
static BOOL fandhe_become_first_responder(id self, SEL __unused _cmd) {
    FandheTextField *field = FandheEnsureTextField((UIView *)self);
    // 前のフォーカス先の未確定文字列・確定済みテキストを引き継がない
    // （#795 計画: 新しいフィールドへフォーカスが移るたびに空へ戻す）。
    field.text = @"";
    fandhe_ios_text_input_reset();
    return [field becomeFirstResponder];
}

static BOOL fandhe_resign_first_responder(id self, SEL __unused _cmd) {
    FandheTextField *field = objc_getAssociatedObject((UIView *)self, kFandheTextFieldKey);
    if (field == nil) {
        return YES;
    }
    return [field resignFirstResponder];
}

void fandhe_ios_text_input_install(void) {
    if (g_installed) {
        return;
    }
    Class viewClass = objc_getClass("WinitUIView");
    if (viewClass == Nil) {
        // winit のビュー階層がまだ構築されていない、または将来クラス名が変わった場合。
        // 呼び出し元（`ios_text_input.rs`）は次フレームの `update` で再試行する。
        return;
    }
    g_installed = YES;

    SEL becomeSel = @selector(becomeFirstResponder);
    SEL resignSel = @selector(resignFirstResponder);
    // 型エンコーディングは `UIResponder` の両セレクタと同一シグネチャ（`BOOL (id, SEL)`）。
    const char *typeEncoding = "B@:";
    class_replaceMethod(viewClass, becomeSel, (IMP)fandhe_become_first_responder, typeEncoding);
    class_replaceMethod(viewClass, resignSel, (IMP)fandhe_resign_first_responder, typeEncoding);
}

void fandhe_ios_text_input_clear(void) {
    if (g_activeField != nil) {
        g_activeField.text = @"";
    }
}

int32_t fandhe_ios_text_input_set_one_time_code_hint(int32_t on) {
    if (![NSThread isMainThread]) {
        return 2;
    }
    @autoreleasepool {
        // 失敗を返す経路では希望状態を変えない（失敗と出たのに後から生成された入力欄へ
        // ヒントが効く、という報告と状態の食い違いを作らない）。
        if (on != 0 && !g_installed) {
            return 3;
        }
        g_oneTimeCodeHint = (on != 0);
        if (g_activeField != nil) {
            FandheApplyContentHint(g_activeField);
            if ([g_activeField isFirstResponder]) {
                [g_activeField reloadInputViews];
            }
        }
        return 0;
    }
}
