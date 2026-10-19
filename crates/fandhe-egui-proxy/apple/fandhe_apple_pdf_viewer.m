// Phase 11 の拡充ループ（親 #107）型サンプル「PDF 表示」: macOS（Issue #469）の実装。
// iOS は b-ios が同じ実装を共用する（CoreGraphics は iOS にもあり、`#if` 分岐は要らない）。
//
// `crates/fandhe-egui-proxy/src/device/apple_pdf_viewer.rs` から C ABI 経由で呼ばれる。
// `build.rs` の macOS / iOS 両分岐が feature `device` 有効時に `cc` でコンパイルし、
// CoreGraphics をリンクする（PDFKit は使わない）。
//
// 契約（同期呼び出し。ワーカースレッドから呼ぶ。UI・main スレッドは不要）:
//   fandhe_apple_pdf_render_first_page(pdf, len, max_side, out, out_cap, w, h, pages) -> int32_t
//     pdf/len: 呼び出しの間だけ有効な PDF のバイト列（コピーしない）。
//     out/out_cap: Rust が確保した出力バッファ。w*h*4 <= out_cap を確かめてから描く。
//     w/h/pages: 成功時だけ書く。pages は UINT32_MAX で飽和。
//     戻り値の表は fandhe_device::pdf_viewer::decode_code が SSOT
//     （0 成功 / 1 非対応 / 2 壊れている / 3 暗号化で開けない / 4 0 ページ / 5 引数・寸法の不正）。
//
// 出力は RGBA・行優先・不透明（白で塗ってから描くので premultiplied と straight は同じ値）。
// CG の資源（context・color space・document・provider）は単一の出口ですべての経路で解放する。
// PDF の中身・画素・エラーはログへ出さない（NSLog 不使用、security.md A09）。
// 実機での表示確認（向き・色・「全 3 ページ」）は #468（人手）。

#include <CoreGraphics/CoreGraphics.h>
#include <math.h>
#include <stddef.h>
#include <stdint.h>

int32_t fandhe_apple_pdf_render_first_page(const uint8_t *pdf, size_t len, uint32_t max_side,
                                           uint8_t *out, size_t out_cap, uint32_t *w, uint32_t *h,
                                           uint32_t *pages) {
    if (pdf == NULL || len == 0 || max_side == 0 || out == NULL || w == NULL || h == NULL ||
        pages == NULL) {
        return 5;
    }
    int32_t code = 2;
    CGDataProviderRef provider = NULL;
    CGPDFDocumentRef doc = NULL;
    CGColorSpaceRef space = NULL;
    CGContextRef ctx = NULL;

    // 解放しないバイト列を包むだけ（Rust の呼び出しの間だけ有効）。
    provider = CGDataProviderCreateWithData(NULL, pdf, len, NULL);
    if (provider == NULL) {
        goto done;
    }
    doc = CGPDFDocumentCreateWithProvider(provider);
    if (doc == NULL) {
        goto done;
    }
    if (CGPDFDocumentIsEncrypted(doc) && !CGPDFDocumentIsUnlocked(doc)) {
        code = 3;
        goto done;
    }
    size_t count = CGPDFDocumentGetNumberOfPages(doc);
    if (count == 0) {
        code = 4;
        goto done;
    }
    CGPDFPageRef page = CGPDFDocumentGetPage(doc, 1);
    if (page == NULL) {
        goto done;
    }
    CGRect box = CGPDFPageGetBoxRect(page, kCGPDFCropBox);
    double bw = box.size.width;
    double bh = box.size.height;
    if (!isfinite(bw) || !isfinite(bh) || bw <= 0 || bh <= 0) {
        code = 5;
        goto done;
    }
    // 長辺を max_side に合わせ、縦横比を保つ（Rust の render_size と同じ式）。
    double scale = (double)max_side / (bw > bh ? bw : bh);
    double fw = round(bw * scale);
    double fh = round(bh * scale);
    uint32_t pw = fw < 1 ? 1 : (fw > max_side ? max_side : (uint32_t)fw);
    uint32_t ph = fh < 1 ? 1 : (fh > max_side ? max_side : (uint32_t)fh);
    if ((size_t)pw * (size_t)ph * 4 > out_cap) {
        code = 5;
        goto done;
    }
    space = CGColorSpaceCreateDeviceRGB();
    if (space == NULL) {
        goto done;
    }
    ctx = CGBitmapContextCreate(out, pw, ph, 8, (size_t)pw * 4, space,
                                kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big);
    if (ctx == NULL) {
        goto done;
    }
    CGContextSetRGBFillColor(ctx, 1, 1, 1, 1);
    CGContextFillRect(ctx, CGRectMake(0, 0, pw, ph));
    CGContextScaleCTM(ctx, (double)pw / bw, (double)ph / bh);
    CGContextTranslateCTM(ctx, -box.origin.x, -box.origin.y);
    CGContextDrawPDFPage(ctx, page);
    *w = pw;
    *h = ph;
    *pages = count > UINT32_MAX ? UINT32_MAX : (uint32_t)count;
    code = 0;

done:
    if (ctx != NULL) {
        CGContextRelease(ctx);
    }
    if (space != NULL) {
        CGColorSpaceRelease(space);
    }
    if (doc != NULL) {
        CGPDFDocumentRelease(doc);
    }
    if (provider != NULL) {
        CGDataProviderRelease(provider);
    }
    return code;
}
