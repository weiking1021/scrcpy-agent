package io.github.weiking1021.scrcpyagent.android;

import android.app.Activity;
import android.content.ComponentName;
import android.content.ServiceConnection;
import android.content.pm.PackageManager;
import android.graphics.Rect;
import android.os.IBinder;
import java.io.IOException;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicInteger;
import rikka.shizuku.Shizuku;

/** Optional Shizuku connection, held only while its mirror is open. */
final class ShizukuWindow implements AutoCloseable {
    private static final AtomicInteger requests = new AtomicInteger(4200);
    private static int connections;
    private final Activity owner;
    private final ScrcpySessions.Entry entry;
    private final int request = requests.incrementAndGet();
    private final Shizuku.UserServiceArgs arguments;
    private final java.util.concurrent.ExecutorService worker = Executors.newSingleThreadExecutor();
    private volatile IWindowControl control;
    private volatile boolean closed;
    private boolean bound;
    private Rect pendingBounds;
    private int pendingTask;
    private boolean resizing;
    private final Shizuku.OnRequestPermissionResultListener permission = this::permissionResult;
    private final Shizuku.OnBinderDeadListener dead = this::binderDied;

    private void permissionResult(int code, int result) {
        owner.runOnUiThread(() -> {
            if (closed || code != request) return;
            if (result == PackageManager.PERMISSION_GRANTED) bind();
            else fail(new SecurityException(owner.getString(R.string.window_shizuku_denied)));
        });
    }

    private void binderDied() {
        owner.runOnUiThread(() -> fail(new IOException(owner.getString(R.string.window_shizuku_unavailable))));
    }
    private final ServiceConnection connection = new ServiceConnection() {
        @Override public void onServiceConnected(ComponentName name, IBinder binder) {
            connected(binder);
        }
        @Override public void onServiceDisconnected(ComponentName name) {
            fail(new IOException(owner.getString(R.string.window_shizuku_unavailable)));
        }
    };

    ShizukuWindow(Activity owner, ScrcpySessions.Entry entry) {
        this.owner = owner; this.entry = entry;
        arguments = new Shizuku.UserServiceArgs(new ComponentName(owner, ShizukuWindowService.class))
            .tag("mirror-windows").daemon(false).debuggable(false).version(BuildConfig.VERSION_CODE);
    }

    synchronized void open() {
        if (closed) return;
        try {
            if (!Shizuku.pingBinder() || Shizuku.getVersion() < 13)
                throw new IOException(owner.getString(R.string.window_shizuku_unavailable));
            Shizuku.addBinderDeadListener(dead);
            if (Shizuku.checkSelfPermission() == PackageManager.PERMISSION_GRANTED) bind();
            else {
                Shizuku.addRequestPermissionResultListener(permission);
                Shizuku.requestPermission(request);
            }
        } catch (Exception error) { fail(error); }
    }

    private synchronized void bind() {
        if (closed || bound) return;
        bound = true;
        connections++;
        try { Shizuku.bindUserService(arguments, connection); }
        catch (Exception error) { fail(error); }
    }

    private synchronized void connected(IBinder binder) {
        if (closed) return;
        control = IWindowControl.Stub.asInterface(binder);
        worker.execute(() -> {
            if (closed) return;
            try { check(control.launch(entry.key, entry.launchToken)); }
            catch (Exception error) { owner.runOnUiThread(() -> fail(error)); }
        });
    }

    synchronized void resize(int task, Rect bounds) {
        if (closed || control == null) return;
        pendingTask = task; pendingBounds = new Rect(bounds);
        if (resizing) return;
        resizing = true;
        worker.execute(this::resizePending);
    }

    private void resizePending() {
        try {
            while (!closed) {
                final Rect bounds;
                final int task;
                synchronized (this) {
                    bounds = pendingBounds; task = pendingTask; pendingBounds = null;
                    if (bounds == null) { resizing = false; return; }
                }
                check(control.resize(task, bounds.left, bounds.top, bounds.right, bounds.bottom));
            }
        } catch (Exception error) { owner.runOnUiThread(() -> fail(error)); }
    }

    private static void check(String error) throws IOException {
        if (error != null) throw new IOException(error);
    }

    private void fail(Exception error) {
        if (!closed) MirrorWindows.fail(owner, entry, error);
    }

    @Override public synchronized void close() {
        if (closed) return;
        closed = true;
        worker.shutdownNow();
        owner.runOnUiThread(() -> {
            Shizuku.removeRequestPermissionResultListener(permission);
            Shizuku.removeBinderDeadListener(dead);
            if (bound) {
                bound = false;
                boolean last = --connections == 0;
                try { Shizuku.unbindUserService(arguments, connection, last); }
                catch (RuntimeException ignored) { /* The service may already have stopped. */ }
            }
        });
    }
}
