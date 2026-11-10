package io.github.weiking1021.scrcpyagent.android;

import android.app.Activity;
import android.app.PictureInPictureParams;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.content.pm.PackageManager;
import android.graphics.PixelFormat;
import android.net.Uri;
import android.provider.Settings;
import android.util.Rational;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;

/** Presentation and window chrome; video decoding and touch mapping remain in MirrorActivity. */
final class MirrorWindow implements AutoCloseable {
    static final int OVERLAY_PERMISSION = 4101;
    private final Activity activity;
    private final ScrcpySessions.Entry entry;
    private final FrameLayout content;
    private final WindowManager manager;
    private View floating;
    private WindowManager.LayoutParams parameters;
    private boolean pipAttempted;
    private boolean closed;

    MirrorWindow(Activity activity, ScrcpySessions.Entry entry, FrameLayout content) {
        this.activity = activity; this.entry = entry; this.content = content;
        manager = (WindowManager)activity.getApplicationContext().getSystemService(android.content.Context.WINDOW_SERVICE);
    }

    void show() {
        try {
            switch (entry.mode) {
                case OVERLAY:
                    if (!Settings.canDrawOverlays(activity)) {
                        activity.setContentView(new FrameLayout(activity));
                        activity.startActivityForResult(new Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                            Uri.parse("package:" + activity.getPackageName())), OVERLAY_PERMISSION);
                    } else showOverlay();
                    break;
                default: activity.setContentView(content);
            }
        } catch (Exception error) { fail(error); }
    }

    void overlayResult() {
        if (closed || activity.isFinishing()) return;
        if (!Settings.canDrawOverlays(activity)) { fail(new SecurityException(activity.getString(R.string.window_overlay_denied))); return; }
        try { showOverlay(); } catch (Exception error) { fail(error); }
    }

    private void showOverlay() {
        var bounds = MirrorWindows.bounds(activity, entry.videoWidth, entry.videoHeight, entry.paddingPx(activity) * 2 + dp(62));
        parameters = new WindowManager.LayoutParams(bounds.width(), bounds.height(),
            WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
            WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL | WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON,
            PixelFormat.TRANSLUCENT);
        parameters.gravity = Gravity.TOP | Gravity.LEFT;
        parameters.x = bounds.left; parameters.y = bounds.top;
        floating = chrome();
        manager.addView(floating, parameters);
        activity.moveTaskToBack(true);
    }

    private View chrome() {
        var root = new LinearLayout(activity);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setBackgroundColor(0xff242424);
        var bar = new LinearLayout(activity);
        var title = new TextView(activity);
        title.setText(entry.request.optString("title", "ScrcpyAgent"));
        title.setTextColor(0xffffffff); title.setGravity(Gravity.CENTER_VERTICAL);
        title.setPadding(dp(10), 0, dp(4), 0);
        bar.addView(title, new LinearLayout.LayoutParams(0, dp(40), 1));
        var close = new Button(activity);
        close.setText("×"); close.setContentDescription(activity.getString(R.string.window_close));
        close.setOnClickListener(v -> activity.finishAndRemoveTask());
        bar.addView(close, new LinearLayout.LayoutParams(dp(48), dp(40)));
        root.addView(bar);
        root.addView(content, new LinearLayout.LayoutParams(-1, 0, 1));
        title.setOnTouchListener(new View.OnTouchListener() {
            private float x, y; private int left, top;
            public boolean onTouch(View view, MotionEvent event) {
                if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
                    x = event.getRawX(); y = event.getRawY(); left = parameters.x; top = parameters.y;
                } else if (event.getActionMasked() == MotionEvent.ACTION_MOVE) {
                    parameters.x = left + Math.round(event.getRawX() - x);
                    parameters.y = top + Math.round(event.getRawY() - y);
                    updateOverlay();
                } else if (event.getActionMasked() == MotionEvent.ACTION_UP) view.performClick();
                return true;
            }
        });
        var resize = new TextView(activity);
        resize.setText("↘"); resize.setTextColor(0xffffffff); resize.setGravity(Gravity.RIGHT);
        resize.setContentDescription(activity.getString(R.string.window_resize));
        root.addView(resize, new LinearLayout.LayoutParams(-1, dp(22)));
        resize.setOnTouchListener(new View.OnTouchListener() {
            private float x; private int width;
            public boolean onTouch(View view, MotionEvent event) {
                if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
                    x = event.getRawX(); width = parameters.width;
                } else if (event.getActionMasked() == MotionEvent.ACTION_MOVE) {
                    var screen = activity.getWindowManager().getMaximumWindowMetrics().getBounds();
                    int extraHeight = entry.paddingPx(activity) * 2 + dp(62);
                    int requestedWidth = Math.max(dp(140), width + Math.round(event.getRawX() - x));
                    parameters.width = MirrorGeometry.fitWidth(Math.min(screen.width(), requestedWidth),
                        screen.height(), entry.videoWidth, entry.videoHeight, extraHeight);
                    parameters.height = MirrorGeometry.heightForWidth(parameters.width,
                        entry.videoWidth, entry.videoHeight, extraHeight);
                    parameters.x = Math.max(screen.left, Math.min(parameters.x, screen.right - parameters.width));
                    parameters.y = Math.max(screen.top, Math.min(parameters.y, screen.bottom - parameters.height));
                    updateOverlay();
                } else if (event.getActionMasked() == MotionEvent.ACTION_UP) view.performClick();
                return true;
            }
        });
        return root;
    }

    void size(int width, int height) {
        if (closed) return;
        try {
            if (entry.mode == MirrorWindows.Mode.FREEFORM) {
                int orientation = width > height ? ActivityInfo.SCREEN_ORIENTATION_LANDSCAPE : ActivityInfo.SCREEN_ORIENTATION_PORTRAIT;
                if (activity.getRequestedOrientation() != orientation) activity.setRequestedOrientation(orientation);
                return;
            }
            int chrome = entry.mode == MirrorWindows.Mode.OVERLAY ? dp(62)
                : Math.max(0, activity.getWindow().getDecorView().getHeight() - content.getHeight());
            var bounds = MirrorWindows.bounds(activity, width, height, entry.paddingPx(activity) * 2 + chrome);
            switch (entry.mode) {
                case OVERLAY:
                    if (parameters != null) {
                        parameters.width = bounds.width(); parameters.height = bounds.height();
                        var screen = activity.getWindowManager().getMaximumWindowMetrics().getBounds();
                        parameters.x = Math.max(screen.left, Math.min(parameters.x, screen.right - parameters.width));
                        parameters.y = Math.max(screen.top, Math.min(parameters.y, screen.bottom - parameters.height));
                        updateOverlay();
                    }
                    break;
                case PIP:
                    if (!activity.getPackageManager().hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE))
                        throw new UnsupportedOperationException(activity.getString(R.string.window_pip_unsupported));
                    var pip = new PictureInPictureParams.Builder().setAspectRatio(pipRatio(bounds.width(), bounds.height())).build();
                    activity.setPictureInPictureParams(pip);
                    if (!pipAttempted) {
                        pipAttempted = true;
                        if (!activity.enterPictureInPictureMode(pip)) throw new IllegalStateException("PiP was refused by the system");
                    }
                    break;
                case SHIZUKU:
                    if (entry.windowControl != null) entry.windowControl.resize(activity.getTaskId(), bounds);
                    break;
                default: break;
            }
        } catch (Exception error) { fail(error); }
    }

    static Rational pipRatio(int width, int height) {
        float ratio = (float)width / height;
        if (ratio < 1f / 2.39f) return new Rational(100, 239);
        if (ratio > 2.39f) return new Rational(239, 100);
        return new Rational(width, height);
    }

    private int dp(int value) { return Math.round(value * activity.getResources().getDisplayMetrics().density); }
    private void updateOverlay() { if (!closed && floating != null) manager.updateViewLayout(floating, parameters); }
    private void fail(Exception error) { MirrorWindows.fail(activity, entry, error); }
    public void close() {
        closed = true;
        if (floating != null && floating.isAttachedToWindow()) manager.removeViewImmediate(floating);
        floating = null;
    }
}
