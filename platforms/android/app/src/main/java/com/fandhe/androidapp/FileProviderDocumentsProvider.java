package com.fandhe.androidapp;

import android.content.Context;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.CancellationSignal;
import android.os.Handler;
import android.os.Looper;
import android.os.ParcelFileDescriptor;
import android.provider.DocumentsContract;
import android.provider.DocumentsContract.Document;
import android.provider.DocumentsContract.Root;
import android.provider.DocumentsProvider;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructStat;
import android.webkit.MimeTypeMap;

import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.security.SecureRandom;

// OS のファイル画面（Files アプリ・他アプリの保存先選択）にアプリ専用フォルダを出す
// DocumentsProvider（Issue #1020、親 #194）。system（MANAGE_DOCUMENTS 保持者）だけが bind する。
// Rust の `fandhe_device::file_provider::DirFileProvider`（`device/android_file_provider.rs`）と
// 同じフォルダ `getFilesDir()/file-provider` を、同じ配置 `<id>/<表示名>` と同じ規則で触る。
//
// 方式（契約は `fandhe_device::file_provider` のモジュール doc）:
// - 1 書類 = 再利用しない書類 ID 1 つ。document ID は ID で、表示名は識別に使わない。
// - 作成は mkdir（既にあれば失敗）で一意性を OS に確かめさせる。消した ID は二度と使わない。
//   古い URI は解決できず、同じ表示名で作り直しても権限は新しい書類に及ばない。
//   このため世代表も、Rust 側との共通の排他も持たない。
// - 作りかけ（空のフォルダ・2 項目以上・通常ファイル以外）は一覧に出さず失敗にもしない。
//
// 守る範囲: パストラバーサルの拒否、上書きしないこと、一覧の I/O 失敗を 0 件の成功にしないこと、
// 削除失敗の握りつぶし禁止、名前・ID・パスをログに出さないこと。
// 守らない範囲: 他アプリが書き込み中の途中サイズ、拡張子の無い名前の MIME（octet-stream）、
// lstat と open の間の差し替え（私有フォルダに項目を作れるのは自プロセスだけ）。
public final class FileProviderDocumentsProvider extends DocumentsProvider {

    // Rust 側 `fandhe_device::file_provider` の FOLDER_NAME / MAX_NAME_BYTES / MAX_ID_LEN と一致させる。
    static final String FOLDER_NAME = "file-provider";
    private static final int MAX_NAME_BYTES = 255;
    private static final int MAX_ID_LEN = 64;
    private static final int MAX_ID_TRIES = 8;
    private static final String ROOT_ID = "root";
    // ID の規則（英数字のみ）に通らない値にして、書類の ID と衝突させない。
    private static final String ROOT_DOC_ID = "_root";
    private static final String[] ROOT_COLUMNS = {
        Root.COLUMN_ROOT_ID, Root.COLUMN_FLAGS, Root.COLUMN_TITLE,
        Root.COLUMN_DOCUMENT_ID, Root.COLUMN_MIME_TYPES
    };
    private static final String[] DOC_COLUMNS = {
        Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE,
        Document.COLUMN_SIZE, Document.COLUMN_LAST_MODIFIED, Document.COLUMN_FLAGS
    };
    private static final SecureRandom RANDOM = new SecureRandom();

    private static String authority(Context c) {
        return c.getPackageName() + ".documents";
    }

    private File root() {
        return new File(getContext().getFilesDir(), FOLDER_NAME);
    }

    // Rust `validate_name` と同じ表。空・"."・".."・"/"・NUL・制御文字・先頭 "."・255 バイト超・
    // 不正なサロゲート（UTF-8 にできない）を拒否する。
    static boolean validName(String n) {
        if (n == null || n.isEmpty() || n.startsWith(".") || n.equals("..")) {
            return false;
        }
        for (int i = 0; i < n.length(); ) {
            int cp = n.codePointAt(i);
            i += Character.charCount(cp);
            if (cp == '/' || Character.isISOControl(cp)
                    || (cp >= 0xD800 && cp <= 0xDFFF)) {
                return false;
            }
        }
        return n.getBytes(java.nio.charset.StandardCharsets.UTF_8).length <= MAX_NAME_BYTES;
    }

    // Rust `validate_id` と同じ表。ASCII 英数字のみ・空でない・MAX_ID_LEN 以下。
    static boolean validId(String id) {
        if (id == null || id.isEmpty() || id.length() > MAX_ID_LEN) {
            return false;
        }
        for (int i = 0; i < id.length(); i++) {
            char c = id.charAt(i);
            boolean ok = (c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z');
            if (!ok) {
                return false;
            }
        }
        return true;
    }

    private static String newId() {
        byte[] b = new byte[16];
        RANDOM.nextBytes(b);
        StringBuilder sb = new StringBuilder(32);
        for (byte x : b) {
            sb.append(Character.forDigit((x >> 4) & 0xF, 16)).append(Character.forDigit(x & 0xF, 16));
        }
        return sb.toString();
    }

    private static StructStat lstat(File f) throws ErrnoException {
        return Os.lstat(f.getPath());
    }

    // ID サブフォルダ内の書類ファイルを返す。作りかけ・規則外は null。lstat が ENOENT 以外で
    // 失敗したら ErrnoException（呼び出し側が 0 件の成功にせず失敗にする）。
    private static File docFile(File sub) throws ErrnoException {
        StructStat st;
        try {
            st = lstat(sub);
        } catch (ErrnoException e) {
            if (e.errno == OsConstants.ENOENT) {
                return null;
            }
            throw e;
        }
        if (!OsConstants.S_ISDIR(st.st_mode)) {
            return null;
        }
        String[] names = sub.list();
        if (names == null) {
            throw new ErrnoException("list", OsConstants.EIO);
        }
        if (names.length != 1 || !validName(names[0])) {
            return null;
        }
        File f = new File(sub, names[0]);
        try {
            if (!OsConstants.S_ISREG(lstat(f).st_mode)) {
                return null;
            }
        } catch (ErrnoException e) {
            if (e.errno == OsConstants.ENOENT) {
                return null;
            }
            throw e;
        }
        return f;
    }

    // document ID を書類ファイルへ解決する。消えた ID・規則外・作りかけは FileNotFoundException。
    private File resolve(String docId) throws FileNotFoundException {
        if (!validId(docId)) {
            throw new FileNotFoundException();
        }
        try {
            File f = docFile(new File(root(), docId));
            if (f == null) {
                throw new FileNotFoundException();
            }
            return f;
        } catch (ErrnoException e) {
            throw new FileNotFoundException();
        }
    }

    private static String mimeOf(String name) {
        int dot = name.lastIndexOf('.');
        if (dot >= 0 && dot < name.length() - 1) {
            String m = MimeTypeMap.getSingleton()
                    .getMimeTypeFromExtension(name.substring(dot + 1).toLowerCase(java.util.Locale.ROOT));
            if (m != null) {
                return m;
            }
        }
        return "application/octet-stream";
    }

    private void addDoc(MatrixCursor c, String id, File f) {
        c.newRow()
                .add(Document.COLUMN_DOCUMENT_ID, id)
                .add(Document.COLUMN_DISPLAY_NAME, f.getName())
                .add(Document.COLUMN_MIME_TYPE, mimeOf(f.getName()))
                .add(Document.COLUMN_SIZE, f.length())
                .add(Document.COLUMN_LAST_MODIFIED, f.lastModified())
                .add(Document.COLUMN_FLAGS, Document.FLAG_SUPPORTS_WRITE | Document.FLAG_SUPPORTS_DELETE);
    }

    // ルート直下の一覧が変わったことを通知する。Rust の put/delete 後と Java 側の変更後に呼ぶ。
    // 投げられたら true、例外は false（成功へ倒さない）。
    static boolean notifyChanged(Context ctx) {
        try {
            Uri uri = DocumentsContract.buildChildDocumentsUri(authority(ctx), ROOT_DOC_ID);
            ctx.getContentResolver().notifyChange(uri, null);
            return true;
        } catch (RuntimeException e) {
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
                .add(Root.COLUMN_MIME_TYPES, "*/*");
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
                    .add(Document.COLUMN_SIZE, 0L)
                    .add(Document.COLUMN_LAST_MODIFIED, null)
                    .add(Document.COLUMN_FLAGS, Document.FLAG_DIR_SUPPORTS_CREATE);
            return c;
        }
        addDoc(c, documentId, resolve(documentId));
        return c;
    }

    @Override
    public Cursor queryChildDocuments(String parentDocumentId, String[] projection, String sortOrder)
            throws FileNotFoundException {
        if (!ROOT_DOC_ID.equals(parentDocumentId)) {
            throw new FileNotFoundException();
        }
        MatrixCursor c = new MatrixCursor(projection != null ? projection : DOC_COLUMNS);
        c.setNotificationUri(getContext().getContentResolver(),
                DocumentsContract.buildChildDocumentsUri(authority(getContext()), ROOT_DOC_ID));
        File dir = root();
        if (!dir.exists()) {
            return c;
        }
        String[] ids = dir.list();
        if (ids == null) {
            // 読めないフォルダを 0 件の成功にしない。
            throw new FileNotFoundException();
        }
        for (String id : ids) {
            if (!validId(id)) {
                continue;
            }
            try {
                File f = docFile(new File(dir, id));
                if (f != null) {
                    addDoc(c, id, f);
                }
            } catch (ErrnoException e) {
                throw new FileNotFoundException();
            }
        }
        return c;
    }

    @Override
    public ParcelFileDescriptor openDocument(String documentId, String mode, CancellationSignal signal)
            throws FileNotFoundException {
        File f = resolve(documentId);
        int pfdMode = ParcelFileDescriptor.parseMode(mode);
        if (!mode.contains("w")) {
            return ParcelFileDescriptor.open(f, pfdMode);
        }
        // 書き込みは閉じたときに一覧の更新を通知する（推測タイマーは使わない）。
        final Context ctx = getContext();
        try {
            return ParcelFileDescriptor.open(f, pfdMode, new Handler(Looper.getMainLooper()),
                    e -> notifyChanged(ctx));
        } catch (IOException e) {
            throw new FileNotFoundException();
        }
    }

    @Override
    public String createDocument(String parentDocumentId, String mimeType, String displayName)
            throws FileNotFoundException {
        // 型は保存しない（メタデータの別ファイルを持たない契約）。MIME は拡張子から決める。
        if (!ROOT_DOC_ID.equals(parentDocumentId)
                || Document.MIME_TYPE_DIR.equals(mimeType)
                || !validName(displayName)) {
            throw new FileNotFoundException();
        }
        File dir = root();
        if (!dir.isDirectory() && !dir.mkdirs() && !dir.isDirectory()) {
            throw new FileNotFoundException();
        }
        for (int i = 0; i < MAX_ID_TRIES; i++) {
            String id = newId();
            File sub = new File(dir, id);
            if (!sub.mkdir()) {
                if (sub.exists()) {
                    continue;
                }
                throw new FileNotFoundException();
            }
            File f = new File(sub, displayName);
            boolean created;
            try {
                created = f.createNewFile();
            } catch (IOException e) {
                created = false;
            }
            if (!created) {
                // 作りかけはファイル→サブフォルダの順に消す。消せなくても一覧には出ない。
                f.delete();
                sub.delete();
                throw new FileNotFoundException();
            }
            notifyChanged(getContext());
            return id;
        }
        throw new FileNotFoundException();
    }

    @Override
    public void deleteDocument(String documentId) throws FileNotFoundException {
        File f = resolve(documentId);
        File sub = f.getParentFile();
        // ファイル→ID のサブフォルダの順。失敗は握りつぶさない。
        if (!f.delete() || !sub.delete()) {
            throw new FileNotFoundException();
        }
        notifyChanged(getContext());
    }
}
