package io.github.weiking1021.scrcpyagent.android;

import android.app.Activity;
import android.app.ActivityOptions;
import android.content.Intent;
import android.graphics.Rect;
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

        Entry(JSONObject request, Consumer<JSONObject> input) throws Exception {
            this.key = request.getString("key");
            this.request = request;
            this.input = input;
        }

        void end(Exception error) {
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
                var intent = new Intent(owner, MirrorActivity.class)
                    .setData(Uri.parse("scrcpy-agent://session/" + Uri.encode(entry.key)))
                    .putExtra("sessionKey", entry.key)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_MULTIPLE_TASK);
                var metrics = owner.getResources().getDisplayMetrics();
                int width = metrics.widthPixels * 2 / 3, height = metrics.heightPixels * 4 / 5;
                int left = (metrics.widthPixels - width) / 2, top = (metrics.heightPixels - height) / 2;
                var options = ActivityOptions.makeBasic();
                // A public launch hint: the OS decides whether freeform windows are supported.
                options.setLaunchBounds(new Rect(left, top, left + width, top + height));
                owner.startActivity(intent, options.toBundle());
            } catch (Exception error) { entry.end(error); }
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
