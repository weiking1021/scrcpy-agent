package io.github.weiking1021.scrcpyagent.android;

import android.app.Activity;
import android.app.ActivityOptions;
import android.graphics.Rect;
import android.os.Bundle;
import android.widget.Toast;

/** Window placement only; all presentations share the same mirror session. */
final class MirrorWindows {
    enum Mode { FREEFORM, OVERLAY, PIP, SHIZUKU }

    static Rect bounds(Activity owner, int videoWidth, int videoHeight, int extraHeight) {
        return fit(owner.getWindowManager().getMaximumWindowMetrics().getBounds(), videoWidth, videoHeight, extraHeight);
    }

    static Rect fit(Rect screen, int videoWidth, int videoHeight, int extraHeight) {
        int width = MirrorGeometry.fitWidth(Math.max(1, screen.width() * 3 / 4), Math.max(1, screen.height() * 3 / 4),
            videoWidth, videoHeight, extraHeight);
        int height = MirrorGeometry.heightForWidth(width, videoWidth, videoHeight, extraHeight);
        int left = screen.left + (screen.width() - width) / 2, top = screen.top + (screen.height() - height) / 2;
        return new Rect(left, top, left + width, top + height);
    }

    static Bundle options(Activity owner, ScrcpySessions.Entry entry) {
        var options = ActivityOptions.makeBasic();
        if (entry.mode == Mode.FREEFORM) options.setLaunchBounds(bounds(owner,
            entry.videoWidth, entry.videoHeight, entry.paddingPx(owner) * 2));
        var bundle = options.toBundle();
        // AOSP ActivityOptions.KEY_LAUNCH_WINDOWING_MODE. The system may ignore this request.
        if (entry.mode == Mode.FREEFORM) bundle.putInt("android.activity.windowingMode", 5);
        return bundle;
    }

    static void fail(Activity owner, ScrcpySessions.Entry entry, Exception error) {
        entry.end(error);
        if (entry.session != null) entry.session.stop();
        var activity = entry.activity;
        if (activity != null) activity.runOnUiThread(activity::finishAndRemoveTask);
        owner.runOnUiThread(() -> Toast.makeText(owner,
            owner.getString(R.string.window_failed, error.getMessage()), Toast.LENGTH_LONG).show());
    }
}
