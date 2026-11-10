package io.github.weiking1021.scrcpyagent.android;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;
import android.provider.OpenableColumns;
import android.webkit.MimeTypeMap;
import android.widget.Toast;
import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.UUID;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.Executors;
import org.json.JSONArray;
import org.json.JSONObject;

/** Android URI grants and staging only. Request policy and UI stay in the shared core. */
final class AndroidShares implements AutoCloseable {
    private static final String CONSUMED = BuildConfig.APPLICATION_ID + ".shareConsumed";
    private final Activity activity;
    private final File root;
    private final Runnable changed;
    private final java.util.concurrent.ExecutorService worker = Executors.newSingleThreadExecutor();
    private final ConcurrentLinkedQueue<JSONObject> ready = new ConcurrentLinkedQueue<>();
    private volatile java.io.InputStream copying;
    private volatile boolean closed;

    AndroidShares(Activity activity, Runnable changed) {
        this.activity = activity;
        this.changed = changed;
        root = new File(activity.getCacheDir(), "incoming-shares");
        // Only our own abandoned imports; never the provider's original files.
        remove(root);
    }

    void receive(Intent intent) {
        try { accept(intent); } catch (RuntimeException error) { failed(error); }
    }

    private void accept(Intent intent) {
        if (closed || intent == null || intent.getComponent() == null || intent.getBooleanExtra(CONSUMED, false)) return;
        String action = ShareImport.actionFor(intent.getComponent().getClassName());
        if (action == null || !(Intent.ACTION_SEND.equals(intent.getAction()) || Intent.ACTION_SEND_MULTIPLE.equals(intent.getAction()))) return;
        intent.putExtra(CONSUMED, true);
        var uris = new ArrayList<Uri>();
        if (Intent.ACTION_SEND.equals(intent.getAction())) {
            Uri uri = intent.getParcelableExtra(Intent.EXTRA_STREAM);
            if (uri != null) uris.add(uri);
        } else {
            ArrayList<Uri> streams = intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM);
            if (streams != null) uris.addAll(streams);
        }
        // Some providers put their grants and streams in ClipData only.
        if (uris.isEmpty() && intent.getClipData() != null) {
            for (int index = 0; index < intent.getClipData().getItemCount(); index++) {
                Uri uri = intent.getClipData().getItemAt(index).getUri();
                if (uri != null) uris.add(uri);
            }
        }
        worker.execute(() -> stage(action, uris));
    }

    private void stage(String action, ArrayList<Uri> uris) {
        if (closed) return;
        String batch = UUID.randomUUID().toString();
        File directory = new File(root, batch);
        try {
            if (uris.isEmpty()) throw new IOException(activity.getString(R.string.share_no_files));
            if (!directory.mkdirs()) throw new IOException("Could not create share cache");
            var files = new JSONArray();
            for (int index = 0; index < uris.size(); index++) {
                if (Thread.currentThread().isInterrupted()) throw new IOException("Import cancelled");
                Uri uri = uris.get(index);
                if (uri == null || !"content".equals(uri.getScheme()))
                    throw new IOException("Unsupported shared URI");
                var provider = activity.getPackageManager().resolveContentProvider(uri.getAuthority(), 0);
                if (provider != null && provider.applicationInfo.uid == android.os.Process.myUid())
                    throw new IOException("An external share cannot read this app's own provider");
                // One subfolder per input preserves identical source names without overwriting.
                File folder = new File(directory, Integer.toString(index));
                if (!folder.mkdir()) throw new IOException("Could not create file cache");
                File destination = new File(folder, displayName(uri));
                try (var input = activity.getContentResolver().openInputStream(uri); var output = Files.newOutputStream(destination.toPath())) {
                    if (input == null) throw new IOException("The shared file cannot be read");
                    copying = input;
                    byte[] buffer = new byte[64 * 1024];
                    int count;
                    while ((count = input.read(buffer)) != -1) {
                        if (Thread.currentThread().isInterrupted()) throw new IOException("Import cancelled");
                        output.write(buffer, 0, count);
                    }
                } finally {
                    copying = null;
                }
                files.put(destination.getAbsolutePath());
            }
            ready.add(new JSONObject().put("batch", batch).put("action", action).put("files", files));
            activity.runOnUiThread(changed);
        } catch (Exception error) {
            remove(directory);
            failed(error);
        }
    }

    private void failed(Exception error) {
        if (!closed) activity.runOnUiThread(() -> Toast.makeText(activity,
            activity.getString(R.string.share_failed, error.getMessage() == null ? error.getClass().getSimpleName() : error.getMessage()),
            Toast.LENGTH_LONG).show());
    }

    private String displayName(Uri uri) throws IOException {
        String name = null;
        try (var cursor = activity.getContentResolver().query(uri, new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) name = cursor.getString(0);
        } catch (IllegalArgumentException | UnsupportedOperationException ignored) { /* Metadata is optional. */ }
        if (name == null || name.isEmpty()) {
            String mime = activity.getContentResolver().getType(uri);
            String extension = mime == null ? null : MimeTypeMap.getSingleton().getExtensionFromMimeType(mime);
            name = "shared-file" + (extension == null ? "" : "." + extension);
        }
        return ShareImport.checkedName(name);
    }

    JSONArray take() {
        var result = new JSONArray();
        JSONObject next;
        while ((next = ready.poll()) != null) result.put(next);
        return result;
    }

    void release(String batch) {
        if (closed) return;
        if (!batch.matches("[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"))
            throw new IllegalArgumentException("Invalid share batch");
        remove(new File(root, batch));
    }

    private static void remove(File path) {
        // Staging contains only directories and copies created by this class.
        File[] children = path.listFiles();
        if (children != null) for (File child : children) remove(child);
        path.delete();
    }

    @Override public void close() {
        closed = true;
        worker.shutdownNow();
        var input = copying;
        if (input != null) try { input.close(); } catch (IOException ignored) { }
        // The worker removes an interrupted import; ready copies belong to this app too.
        ready.clear();
        remove(root);
    }
}
