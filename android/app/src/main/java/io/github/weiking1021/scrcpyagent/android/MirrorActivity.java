package io.github.weiking1021.scrcpyagent.android;

import android.app.Activity;
import android.os.Bundle;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.widget.FrameLayout;
import android.widget.TextView;
import org.json.JSONObject;

/** A resizable native surface for one shared-core session. Window chrome belongs to Android. */
public class MirrorActivity extends Activity implements SurfaceHolder.Callback {
    private ScrcpySessions.Entry entry;
    private SurfaceView video;
    private TextView status;
    private int videoWidth, videoHeight;
    private MirrorWindow presentation;

    @Override public void onCreate(Bundle state) {
        try { entry = ScrcpySessions.get(getIntent().getStringExtra("sessionKey")); }
        catch (Exception error) { super.onCreate(state); finishAndRemoveTask(); return; }
        if (entry.ended.isDone()) { super.onCreate(state); finishAndRemoveTask(); return; }
        super.onCreate(state);
        entry.activity = this;
        videoWidth = entry.videoWidth; videoHeight = entry.videoHeight;
        setTitle(entry.request.optString("title"));
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        getWindow().setDecorFitsSystemWindows(false);
        var frame = new FrameLayout(this);
        frame.setBackgroundColor(android.graphics.Color.BLACK);
        int padding = entry.paddingPx(this);
        frame.setPadding(0, padding, 0, padding);
        frame.setOnApplyWindowInsetsListener((view, insets) -> WindowInsets.CONSUMED);
        video = new SurfaceView(this) {
            @Override public boolean onTouchEvent(MotionEvent event) {
                if (event.getActionMasked() == MotionEvent.ACTION_UP) performClick();
                return touch(event);
            }
            @Override public boolean performClick() { super.performClick(); return true; }
        };
        video.getHolder().addCallback(this);
        frame.addView(video, new FrameLayout.LayoutParams(-1, -1, Gravity.CENTER));
        frame.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> fit(frame));
        status = new TextView(this);
        status.setTextColor(android.graphics.Color.WHITE);
        status.setGravity(Gravity.CENTER);
        status.setText(R.string.connecting);
        frame.addView(status, new FrameLayout.LayoutParams(-1, -1));
        presentation = new MirrorWindow(this, entry, frame);
        presentation.show();
        if (entry.ready.isDone()) status.setVisibility(View.GONE);
    }

    private boolean touch(MotionEvent event) {
        if (entry == null || entry.session == null || !entry.session.hasControl()
            || videoWidth == 0 || videoHeight == 0 || video.getWidth() == 0 || video.getHeight() == 0) return true;
        int action = event.getActionMasked();
        int first = event.getActionIndex(), last = first + 1;
        if (action == MotionEvent.ACTION_MOVE || action == MotionEvent.ACTION_CANCEL) { first = 0; last = event.getPointerCount(); }
        int mapped;
        switch (action) {
            case MotionEvent.ACTION_DOWN: case MotionEvent.ACTION_POINTER_DOWN: mapped = 0; break;
            case MotionEvent.ACTION_UP: case MotionEvent.ACTION_POINTER_UP: case MotionEvent.ACTION_CANCEL: mapped = 1; break;
            case MotionEvent.ACTION_MOVE: mapped = 2; break;
            default: return true;
        }
        for (int index = first; index < last; index++) {
            int x = Math.max(0, Math.min(videoWidth - 1, (int)(event.getX(index) * videoWidth / video.getWidth())));
            int y = Math.max(0, Math.min(videoHeight - 1, (int)(event.getY(index) * videoHeight / video.getHeight())));
            try {
                entry.input.accept(new JSONObject().put("key", entry.key).put("kind", "touch").put("action", mapped)
                    .put("pointer", event.getPointerId(index)).put("x", x).put("y", y)
                    .put("width", videoWidth).put("height", videoHeight)
                    .put("pressure", mapped == 1 ? 0 : event.getPressure(index)));
            } catch (Exception error) { entry.end(error); finishAndRemoveTask(); }
        }
        return true;
    }

    private void fit(FrameLayout frame) {
        if (videoWidth == 0 || videoHeight == 0) return;
        int availableWidth = frame.getWidth() - frame.getPaddingLeft() - frame.getPaddingRight();
        int availableHeight = frame.getHeight() - frame.getPaddingTop() - frame.getPaddingBottom();
        float scale = Math.min((float)availableWidth / videoWidth, (float)availableHeight / videoHeight);
        var layout = (FrameLayout.LayoutParams)video.getLayoutParams();
        int width = Math.max(1, Math.round(videoWidth * scale)), height = Math.max(1, Math.round(videoHeight * scale));
        if (layout.width != width || layout.height != height) {
            layout.width = width; layout.height = height; video.setLayoutParams(layout);
        }
    }

    @Override public void surfaceCreated(SurfaceHolder holder) {
        if (entry == null) return;
        if (entry.session != null) {
            entry.session.setSurface(holder.getSurface());
            if (entry.session.hasControl()) {
                try { entry.input.accept(new JSONObject().put("key", entry.key).put("kind", "reset_video")); }
                catch (Exception error) { entry.end(error); finishAndRemoveTask(); }
            }
            return;
        }
        var active = entry;
        entry.session = new MirrorSession(getApplicationContext(), holder.getSurface(), new SessionListener(active), active.request);
        new Thread(() -> {
            try {
                var address = DeviceBackend.endpoint(active.request.getString("serial"));
                active.session.run(address[0], Integer.parseInt(address[1]));
                active.end(null);
            } catch (Exception error) { active.end(error); }
            active.ui(MirrorActivity::finishAndRemoveTask);
        }, "scrcpy-video").start();
    }

    private static final class SessionListener implements MirrorSession.Listener {
        private final ScrcpySessions.Entry entry;
        SessionListener(ScrcpySessions.Entry entry) { this.entry = entry; }
        public void started() {
            entry.ready.complete(new JSONObject());
            entry.ui(window -> window.status.setVisibility(View.GONE));
        }
        public void size(int width, int height) {
            entry.videoWidth = width; entry.videoHeight = height;
            entry.ui(window -> window.videoSize(width, height));
        }
        public void failed(Exception error) { entry.end(error); entry.ui(MirrorActivity::finishAndRemoveTask); }
    }

    private void videoSize(int width, int height) {
        videoWidth = width; videoHeight = height;
        presentation.size(width, height);
        fit((FrameLayout)video.getParent());
    }

    @Override public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) { }
    @Override public void onConfigurationChanged(android.content.res.Configuration configuration) {
        super.onConfigurationChanged(configuration);
        if (presentation != null && videoWidth > 0 && videoHeight > 0) {
            video.post(() -> {
                if (!isFinishing() && !isDestroyed()) presentation.size(videoWidth, videoHeight);
            });
        }
    }
    @Override public void surfaceDestroyed(SurfaceHolder holder) {
        if (entry != null && entry.activity == this && entry.session != null) entry.session.setSurface(null);
    }
    @Override protected void onActivityResult(int request, int result, android.content.Intent data) {
        super.onActivityResult(request, result, data);
        if (request == MirrorWindow.OVERLAY_PERMISSION && presentation != null) presentation.overlayResult();
    }
    @Override protected void onDestroy() {
        if (presentation != null) presentation.close();
        if (entry != null) {
            if (entry.activity == this) entry.activity = null;
            if (!isChangingConfigurations()) {
                if (entry.session != null) entry.session.stop();
                entry.end(null);
            }
        }
        super.onDestroy();
    }
}
