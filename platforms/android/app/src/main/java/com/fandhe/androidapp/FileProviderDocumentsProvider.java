package com.fandhe.androidapp;

import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.os.CancellationSignal;
import android.os.Handler;
import android.os.Looper;
import android.os.ParcelFileDescriptor;
import android.provider.DocumentsContract;
import android.provider.DocumentsContract.Document;
import android.provider.DocumentsContract.Root;
import android.provider.DocumentsProvider;
import android.system.Os;
import android.system.OsConstants;
import android.webkit.MimeTypeMap;

import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.HashSet;
import java.util.Locale;
import java.util.Set;

/**
 * クラウド保存先として OS のファイル画面（Files アプリ・SAF の選択画面）に出る
 * {@code DocumentsProvider}（Issue #1020、親 #194）。
 *
 * <p>役割: 内部ストレージの {@code files/file-provider/}（平ら、サブフォルダなし）を、ルート 1 件
 * 「Fandhe（この端末内のみ）」として出す。manifest の {@code <provider>} から system が bind し、
 * Activity が無くても同じプロセスで動く。native は要らない。
 *
 * <p>Rust 側（{@code android_file_provider.rs}・{@code fandhe_device::file_provider}）とは同じフォルダを
 * 共有する。名前の規則・重複名・上限は Rust の {@code validate_name}・{@code dedup_name} と同じ表に揃える。
 * Rust が置く・消した後は {@code MainActivity.fileProviderNotifyChanged()} 経由で
 * {@link #notifyChanged} を呼び、OS のファイル画面へ知らせる。
 *
 * <p>守る範囲: 名前の検証によるパスの閉じ込め、通常ファイル以外（シンボリックリンク等）を出さない、
 * 既存ファイルを上書きしない、読めない一覧を 0 件にしない（失敗）、書き込み完了を OnCloseListener で知る、
 * 名前・パス・例外文をログに出さない。守らない範囲: lstat と open の間の差し替え（私有フォルダで項目を
 * 作れるのは自プロセスだけ）、容量上限、サブフォルダ・名前変更・コピー・検索・最近使ったもの。
 */
public final class FileProviderDocumentsProvider extends DocumentsProvider {
    // Rust の fandhe_device::file_provider の同名定数と同値にする。
    static final String FOLDER_NAME = "file-provider";
    static final int MAX_NAME_BYTES = 255;
    static final int MAX_DEDUP = 99;

    private static final String ROOT_ID = "root";
    private static final String ROOT_DOC_ID = "root";
    private static final String FILE_PREFIX = ROOT_DOC_ID + "/";

    private static final String[] ROOT_COLUMNS = {
        Root.COLUMN_ROOT_ID, Root.COLUMN_FLAGS, Root.COLUMN_TITLE,
        Root.COLUMN_DOCUMENT_ID, Root.COLUMN_ICON,
    };
    private static final String[] DOC_COLUMNS = {
        Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE,
        Document.COLUMN_SIZE, Document.COLUMN_LAST_MODIFIED, Document.COLUMN_FLAGS,
    };

    private static String authority(Context c) {
        return c.getPackageName() + ".documents";
    }

    private static File dir(Context c) {
        return new File(c.getFilesDir(), FOLDER_NAME);
    }

    /** Rust の validate_name と同じ表。対になっていないサロゲートも拒否する。 */
    static boolean validName(String name) {
        if (name == null || name.isEmpty() || name.startsWith(".")) return false;
        for (int i = 0; i < name.length(); ) {
            int cp = name.codePointAt(i);
            if (cp == '/' || Character.isISOControl(cp)) return false;
            if (cp >= 0xD800 && cp <= 0xDFFF) return false;
            i += Character.charCount(cp);
        }
        return name.getBytes(StandardCharsets.UTF_8).length <= MAX_NAME_BYTES;
    }

    /** Rust の dedup_name と同じ。n == 0 は元の名前、それ以外は「名前 (n).拡張子」。 */
    static String dedupName(String base, int n) {
        if (n == 0) return base;
        int i = base.lastIndexOf('.');
        return i < 0
                ? String.format(Locale.ROOT, "%s (%d)", base, n)
                : String.format(Locale.ROOT, "%s (%d)%s", base.substring(0, i), n, base.substring(i));
    }

    /**
     * doc id（{@code root/<世代>/<名前>}）から検証済みの名前を取り出す。ルート・不正な id・現在の世代と
     * 一致しない（削除後に同名で作り直されたファイルの旧 id）は FileNotFoundException。
     */
    private String nameOf(String docId) throws FileNotFoundException {
        if (docId == null || !docId.startsWith(FILE_PREFIX)) throw new FileNotFoundException();
        String rest = docId.substring(FILE_PREFIX.length());
        int i = rest.indexOf('/');
        if (i <= 0) throw new FileNotFoundException();
        String name = rest.substring(i + 1);
        if (!validName(name) || !rest.substring(0, i).equals(String.valueOf(genOf(getContext(), name, false)))) {
            throw new FileNotFoundException();
        }
        return name;
    }

    private static boolean isRegular(File f) {
        try {
            return OsConstants.S_ISREG(Os.lstat(f.getPath()).st_mode);
        } catch (Exception e) {
            return false;
        }
    }

    private File regularFile(String docId) throws FileNotFoundException {
        return regularFile(docId, nameOf(docId));
    }

    private File regularFile(String docId, String name) throws FileNotFoundException {
        File f = new File(dir(getContext()), name);
        if (!isRegular(f)) throw new FileNotFoundException();
        return f;
    }

    private static String mimeOf(Context c, String name) {
        int i = name.lastIndexOf('.');
        if (i >= 0) {
            String m = MimeTypeMap.getSingleton()
                    .getMimeTypeFromExtension(name.substring(i + 1).toLowerCase(Locale.ROOT));
            if (m != null) return m;
        }
        // 拡張子で決まらないときは createDocument で指定された型を返す。
        return prefs(c).getString(MIME_KEY + name, "application/octet-stream");
    }

    private static void addFile(Context ctx, MatrixCursor c, File f, String name) {
        c.newRow()
                .add(Document.COLUMN_DOCUMENT_ID, FILE_PREFIX + genOf(ctx, name, true) + "/" + name)
                .add(Document.COLUMN_DISPLAY_NAME, name)
                .add(Document.COLUMN_MIME_TYPE, mimeOf(ctx, name))
                .add(Document.COLUMN_SIZE, f.length())
                .add(Document.COLUMN_LAST_MODIFIED, f.lastModified())
                .add(Document.COLUMN_FLAGS, Document.FLAG_SUPPORTS_WRITE | Document.FLAG_SUPPORTS_DELETE);
    }

    private static final String PREFS = "file_provider_known";
    private static final String GEN_KEY = "gen/";
    private static final String MIME_KEY = "mime/";
    private static final String NEXT_KEY = "next";

    private static SharedPreferences prefs(Context c) {
        return c.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    /**
     * 名前ごとの世代。document ID に含め、削除で破棄して再利用しない（単調増加）。同名を作り直すと
     * 別の ID になるので、旧 ID への URI 権限は新ファイルに及ばない。create == false で未割り当ては 0。
     */
    static synchronized long genOf(Context c, String name, boolean create) {
        SharedPreferences p = prefs(c);
        long g = p.getLong(GEN_KEY + name, 0);
        if (g == 0 && create) {
            g = p.getLong(NEXT_KEY, 1);
            p.edit().putLong(NEXT_KEY, g + 1).putLong(GEN_KEY + name, g).commit();
        }
        return g;
    }

    private static void revokeAccess(Context c, String name, long gen) {
        c.revokeUriPermission(
                DocumentsContract.buildDocumentUri(authority(c), FILE_PREFIX + gen + "/" + name),
                Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
    }

    /**
     * 名前の世代を破棄して旧 URI 権限を失効させる。削除の直前に呼ぶ（Rust 経由を含む）。破棄が済んでから消すので、
     * 通知前にプロセスが終了しても、同名の再作成は新しい世代になり旧権限は引き継がれない。
     */
    static synchronized boolean revokeName(Context c, String name) {
        if (!validName(name)) return false;
        long g = genOf(c, name, false);
        if (g != 0) revokeAccess(c, name, g);
        return prefs(c).edit().remove(GEN_KEY + name).remove(MIME_KEY + name).commit();
    }

    /**
     * 一覧に居ない名前の世代を破棄して権限を失効させ（アプリ外の削除の後始末）、居る名前には世代を割り当てる。
     * 守らない範囲: アプリ外（adb 等）の削除と同名再作成が同期より前に重なる場合。
     */
    static synchronized void revokeDeleted(Context c) {
        String[] names = dir(c).list();
        // 一覧が読めなかったときは何も失効しない。
        if (names == null) return;
        Set<String> now = new HashSet<>();
        for (String n : names) now.add(n);
        SharedPreferences p = prefs(c);
        for (String key : p.getAll().keySet()) {
            if (!key.startsWith(GEN_KEY)) continue;
            String n = key.substring(GEN_KEY.length());
            if (!now.contains(n)) revokeName(c, n);
        }
    }

    /** 子の一覧が変わったことを OS のファイル画面へ知らせる。失敗は false。 */
    static boolean notifyChanged(Context c) {
        try {
            revokeDeleted(c);
            c.getContentResolver().notifyChange(
                    DocumentsContract.buildChildDocumentsUri(authority(c), ROOT_DOC_ID), null);
            return true;
        } catch (Exception e) {
            return false;
        }
    }

    @Override
    public boolean onCreate() {
        return true;
    }

    @Override
    public Cursor queryRoots(String[] projection) {
        MatrixCursor c = new MatrixCursor(projection != null ? projection : ROOT_COLUMNS);
        c.newRow()
                .add(Root.COLUMN_ROOT_ID, ROOT_ID)
                .add(Root.COLUMN_FLAGS, Root.FLAG_LOCAL_ONLY | Root.FLAG_SUPPORTS_CREATE)
                .add(Root.COLUMN_TITLE, getContext().getString(R.string.file_provider_root_title))
                .add(Root.COLUMN_DOCUMENT_ID, ROOT_DOC_ID)
                .add(Root.COLUMN_ICON, android.R.drawable.sym_def_app_icon);
        return c;
    }

    @Override
    public Cursor queryDocument(String documentId, String[] projection) throws FileNotFoundException {
        MatrixCursor c = new MatrixCursor(projection != null ? projection : DOC_COLUMNS);
        if (ROOT_DOC_ID.equals(documentId)) {
            c.newRow()
                    .add(Document.COLUMN_DOCUMENT_ID, ROOT_DOC_ID)
                    .add(Document.COLUMN_DISPLAY_NAME, getContext().getString(R.string.file_provider_root_title))
                    .add(Document.COLUMN_MIME_TYPE, Document.MIME_TYPE_DIR)
                    .add(Document.COLUMN_FLAGS, Document.FLAG_DIR_SUPPORTS_CREATE);
        } else {
            String name = nameOf(documentId);
            addFile(getContext(), c, regularFile(documentId, name), name);
        }
        return c;
    }

    @Override
    public Cursor queryChildDocuments(String parentDocumentId, String[] projection, String sortOrder)
            throws FileNotFoundException {
        if (!ROOT_DOC_ID.equals(parentDocumentId)) throw new FileNotFoundException();
        Context ctx = getContext();
        MatrixCursor c = new MatrixCursor(projection != null ? projection : DOC_COLUMNS);
        File d = dir(ctx);
        revokeDeleted(ctx);
        if (d.exists()) {
            String[] names = d.list();
            // 読めないフォルダは 0 件にせず失敗にする。
            if (names == null) throw new FileNotFoundException();
            for (String n : names) {
                File f = new File(d, n);
                try {
                    if (!OsConstants.S_ISREG(Os.lstat(f.getPath()).st_mode)) continue;
                } catch (Exception e) {
                    // 項目が 1 件でも読めなければ全体を失敗にする（契約）。
                    throw new FileNotFoundException();
                }
                if (validName(n)) addFile(ctx, c, f, n);
            }
        }
        c.setNotificationUri(ctx.getContentResolver(),
                DocumentsContract.buildChildDocumentsUri(authority(ctx), ROOT_DOC_ID));
        return c;
    }

    @Override
    public ParcelFileDescriptor openDocument(String documentId, String mode, CancellationSignal signal)
            throws FileNotFoundException {
        File f = regularFile(documentId);
        int m = ParcelFileDescriptor.parseMode(mode);
        if (mode.indexOf('w') < 0 && mode.indexOf('a') < 0) return ParcelFileDescriptor.open(f, m);
        final Context ctx = getContext();
        try {
            // 書き込み完了は閉じた時点で知り、そこで通知する（推測タイマーは使わない）。
            return ParcelFileDescriptor.open(f, m, new Handler(Looper.getMainLooper()),
                    e -> notifyChanged(ctx));
        } catch (IOException e) {
            throw new FileNotFoundException();
        }
    }

    @Override
    public String createDocument(String parentDocumentId, String mimeType, String displayName)
            throws FileNotFoundException {
        if (!ROOT_DOC_ID.equals(parentDocumentId) || Document.MIME_TYPE_DIR.equals(mimeType)
                || !validName(displayName)) {
            throw new FileNotFoundException();
        }
        Context ctx = getContext();
        File d = dir(ctx);
        revokeDeleted(ctx);
        if (!d.isDirectory() && !d.mkdirs() && !d.isDirectory()) throw new FileNotFoundException();
        for (int n = 0; n <= MAX_DEDUP; n++) {
            String name = dedupName(displayName, n);
            if (!validName(name)) throw new FileNotFoundException();
            try {
                // O_EXCL 相当。既存は上書きせず次の番号へ。
                if (new File(d, name).createNewFile()) {
                    // 新規ファイルは必ず新しい世代の ID にする（旧 ID の権限・型を引き継がない）。
                    revokeName(ctx, name);
                    if (mimeType != null && !"application/octet-stream".equals(mimeType)) {
                        prefs(ctx).edit().putString(MIME_KEY + name, mimeType).commit();
                    }
                    genOf(ctx, name, true);
                    notifyChanged(ctx);
                    return FILE_PREFIX + genOf(ctx, name, false) + "/" + name;
                }
            } catch (IOException e) {
                throw new FileNotFoundException();
            }
        }
        throw new FileNotFoundException();
    }

    @Override
    public void deleteDocument(String documentId) throws FileNotFoundException {
        String name = nameOf(documentId);
        File f = regularFile(documentId, name);
        revokeName(getContext(), name);
        if (!f.delete()) throw new FileNotFoundException();
        notifyChanged(getContext());
    }
}
