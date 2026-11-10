package io.github.weiking1021.scrcpyagent.android;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.Consumer;
import org.json.JSONObject;

/** Native presentation lifetime, keyed by the shared core's session key. */
final class ScrcpySessions {
    private static final ConcurrentHashMap<String, Entry> entries = new ConcurrentHashMap<>();

    static final class Entry {
        final String key;
        final JSONObject request;
        final Consumer<JSONObject> input;
        final CompletableFuture<JSONObject> ready = new CompletableFuture<>();
        final CompletableFuture<JSONObject> ended = new CompletableFuture<>();
        volatile MirrorSession session;
        volatile Activity activity;
        final MirrorWindows.Mode mode;
        volatile ShizukuWindow windowControl;
        volatile int videoWidth, videoHeight;
        final String launchToken = java.util.UUID.randomUUID().toString();
        final java.util.concurrent.atomic.AtomicBoolean tokenUsed = new java.util.concurrent.atomic.AtomicBoolean();

        int paddingPx(android.content.Context context) {
            int dp = Math.max(0, Math.min(128, request.optInt("windowPaddingDp", 16)));
            return Math.round(dp * context.getResources().getDisplayMetrics().density);
        }

        Entry(JSONObject request, Consumer<JSONObject> input) throws Exception {
            this.key = request.getString("key");
            this.request = request;
            this.input = input;
            mode = MirrorWindows.Mode.valueOf(request.getString("windowMode").toUpperCase(java.util.Locale.ROOT));
            videoWidth = request.getInt("screenWidth");
            videoHeight = request.getInt("screenHeight");
            if (videoWidth < 1 || videoHeight < 1 || videoWidth > 16384 || videoHeight > 16384)
                throw new IllegalArgumentException("Invalid remote screen dimensions");
        }

        void ui(java.util.function.Consumer<MirrorActivity> update) {
            new android.os.Handler(android.os.Looper.getMainLooper()).post(() -> {
                if (activity instanceof MirrorActivity window && !window.isFinishing() && !window.isDestroyed()) update.accept(window);
            });
        }

        void end(Exception error) {
            var control = windowControl;
            if (control != null) { windowControl = null; control.close(); }
            if (error != null) ready.completeExceptionally(error);
            else if (!ready.isDone()) ready.completeExceptionally(new java.io.IOException("Session closed before it was ready"));
            try {
                ended.complete(new JSONObject().put("success", error == null).put("tail", error == null ? "" : error.toString()));
            } catch (org.json.JSONException impossible) { ended.completeExceptionally(impossible); }
        }
    }

    static CompletableFuture<JSONObject> start(Activity owner, JSONObject request, Consumer<JSONObject> input) throws Exception {
        var entry = new Entry(request, input);
        if (entries.putIfAbsent(entry.key, entry) != null) throw new IllegalStateException("Session is already open");
        entry.ready.whenComplete((value, error) -> {
            if (error != null) entries.remove(entry.key, entry);
        });
        owner.runOnUiThread(() -> {
            try {
                if (entry.ready.isDone()) return;
                if (entry.mode == MirrorWindows.Mode.SHIZUKU) {
                    var control = new ShizukuWindow(owner, entry);
                    entry.windowControl = control;
                    control.open();
                    return;
                }
                var intent = new Intent(owner, MirrorActivity.class)
                    .setData(Uri.parse("scrcpy-agent://session/" + Uri.encode(entry.key)))
                    .putExtra("sessionKey", entry.key)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_MULTIPLE_TASK);
                owner.startActivity(intent, MirrorWindows.options(owner, entry));
            } catch (Exception error) { MirrorWindows.fail(owner, entry, error); }
        });
        return entry.ready;
    }

    static Entry get(String key) {
        var entry = entries.get(key);
        if (entry == null) throw new IllegalStateException("Unknown session");
        return entry;
    }

    static CompletableFuture<JSONObject> waitFor(String key) {
        var entry = get(key);
        return entry.ended.whenComplete((value, error) -> entries.remove(key, entry));
    }

    static void cancel(String key) {
        var entry = entries.get(key);
        if (entry == null) return;
        if (entry.session != null) entry.session.stop();
        var activity = entry.activity;
        if (activity != null) activity.runOnUiThread(activity::finishAndRemoveTask);
        entry.end(new java.io.IOException("Session canceled"));
    }

    static void close() { for (String key : entries.keySet()) cancel(key); }
}
