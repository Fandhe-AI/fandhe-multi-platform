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

    /** doc id から検証済みの名前を取り出す。ルートや不正な id は FileNotFoundException。 */
    private static String nameOf(String docId) throws FileNotFoundException {
        if (docId == null || !docId.startsWith(FILE_PREFIX)) throw new FileNotFoundException();
        String name = docId.substring(FILE_PREFIX.length());
        if (!validName(name)) throw new FileNotFoundException();
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
        File f = new File(dir(getContext()), nameOf(docId));
        if (!isRegular(f)) throw new FileNotFoundException();
        return f;
    }

    private static String mimeOf(String name) {
        int i = name.lastIndexOf('.');
        if (i >= 0) {
            String m = MimeTypeMap.getSingleton()
                    .getMimeTypeFromExtension(name.substring(i + 1).toLowerCase(Locale.ROOT));
            if (m != null) return m;
        }
        return "application/octet-stream";
    }

    private static void addFile(MatrixCursor c, File f, String name) {
        c.newRow()
                .add(Document.COLUMN_DOCUMENT_ID, FILE_PREFIX + name)
                .add(Document.COLUMN_DISPLAY_NAME, name)
                .add(Document.COLUMN_MIME_TYPE, mimeOf(name))
                .add(Document.COLUMN_SIZE, f.length())
                .add(Document.COLUMN_LAST_MODIFIED, f.lastModified())
                .add(Document.COLUMN_FLAGS, Document.FLAG_SUPPORTS_WRITE | Document.FLAG_SUPPORTS_DELETE);
    }

    private static final String PREFS = "file_provider_known";
    private static final String KEY_NAMES = "names";

    /**
     * 前回の一覧に居て今は無い名前の URI 権限を失効させ、現在の名前を記録する。document ID は名前だけで
     * 決まるため、削除（Rust 経由を含む）後に同名の別ファイルができても旧権限で読み書きされないようにする。
     * 権限は一覧に出た項目にしか付かないので、一覧・作成・削除・通知の各入口で呼べば取りこぼさない。
     * 守らない範囲: 削除から次の同期までの短い間。
     */
    static synchronized void revokeDeleted(Context c) {
        SharedPreferences p = c.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        Set<String> old = p.getStringSet(KEY_NAMES, null);
        Set<String> now = new HashSet<>();
        String[] names = dir(c).list();
        if (names != null) for (String n : names) now.add(n);
        if (old != null) {
            for (String n : old) {
                if (!now.contains(n)) {
                    c.revokeUriPermission(
                            DocumentsContract.buildDocumentUri(authority(c), FILE_PREFIX + n),
                            Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
                }
            }
        }
        // 一覧が読めなかった（names == null）ときは記録を更新しない。
        if (names != null) p.edit().putStringSet(KEY_NAMES, now).commit();
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
            addFile(c, regularFile(documentId), nameOf(documentId));
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
                if (validName(n)) addFile(c, f, n);
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
                    notifyChanged(ctx);
                    return FILE_PREFIX + name;
                }
            } catch (IOException e) {
                throw new FileNotFoundException();
            }
        }
        throw new FileNotFoundException();
    }

    @Override
    public void deleteDocument(String documentId) throws FileNotFoundException {
        File f = regularFile(documentId);
        if (!f.delete()) throw new FileNotFoundException();
        notifyChanged(getContext());
    }
}
