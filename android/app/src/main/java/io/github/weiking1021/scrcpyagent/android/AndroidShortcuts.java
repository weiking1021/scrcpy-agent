package io.github.weiking1021.scrcpyagent.android;

import android.app.Activity;
import android.content.Intent;
import android.content.pm.ShortcutInfo;
import android.content.pm.ShortcutManager;
import android.graphics.drawable.Icon;
import android.net.Uri;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentLinkedQueue;
import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

/** Launcher API and intent delivery only; the shared core opens the remote app. */
final class AndroidShortcuts {
    private static final String ACTION = BuildConfig.APPLICATION_ID + ".OPEN_APP";
    private static final String CONSUMED = BuildConfig.APPLICATION_ID + ".shortcutConsumed";
    private final Activity activity;
    private final ShortcutManager manager;
    private final Runnable changed;
    private final ConcurrentLinkedQueue<JSONObject> pending = new ConcurrentLinkedQueue<>();

    AndroidShortcuts(Activity activity, Runnable changed) {
        this.activity = activity;
        this.manager = activity.getSystemService(ShortcutManager.class);
        this.changed = changed;
    }

    JSONObject available() throws JSONException {
        return new JSONObject().put("supported", manager != null && manager.isRequestPinShortcutSupported());
    }

    private static Uri target(String device, String packageName) {
        return new Uri.Builder().scheme("scrcpy-agent").authority("app")
            .appendPath(device).appendPath(packageName).build();
    }

    JSONObject state(JSONObject data) throws JSONException {
        String id = target(data.getString("device"), data.getString("package")).toString();
        boolean pinned = manager != null && manager.getPinnedShortcuts().stream().anyMatch(s -> s.getId().equals(id));
        return new JSONObject().put("desktop", pinned).put("start_menu", false);
    }

    CompletableFuture<JSONObject> pin(JSONObject data) throws JSONException {
        Uri uri = target(data.getString("device"), data.getString("package"));
        String label = data.getString("label");
        var result = new CompletableFuture<JSONObject>();
        activity.runOnUiThread(() -> {
            try {
                if (manager == null || !manager.isRequestPinShortcutSupported()) {
                    result.complete(new JSONObject().put("accepted", false));
                    return;
                }
                Intent intent = activity.getPackageManager().getLaunchIntentForPackage(activity.getPackageName());
                if (intent == null) throw new IllegalStateException("Launcher activity unavailable");
                intent.setAction(ACTION).setData(uri);
                var shortcut = new ShortcutInfo.Builder(activity, uri.toString())
                    .setShortLabel(label).setLongLabel(label)
                    .setIcon(Icon.createWithResource(activity, activity.getApplicationInfo().icon))
                    .setIntent(intent).build();
                result.complete(new JSONObject().put("accepted", manager.requestPinShortcut(shortcut, null)));
            } catch (Exception error) { result.completeExceptionally(error); }
        });
        return result;
    }

    void receive(Intent intent) {
        if (intent == null || !ACTION.equals(intent.getAction()) || intent.getBooleanExtra(CONSUMED, false)) return;
        Uri uri = intent.getData();
        if (uri == null || !"scrcpy-agent".equals(uri.getScheme()) || !"app".equals(uri.getAuthority())) return;
        var parts = uri.getPathSegments();
        if (parts.size() != 2 || parts.get(0).isEmpty() || parts.get(1).isEmpty()) return;
        try {
            pending.add(new JSONObject().put("device", parts.get(0)).put("package", parts.get(1)));
            intent.putExtra(CONSUMED, true);
            changed.run();
        } catch (JSONException error) { throw new IllegalStateException(error); }
    }

    JSONArray take() {
        var result = new JSONArray();
        JSONObject request;
        while ((request = pending.poll()) != null) result.put(request);
        return result;
    }
}
