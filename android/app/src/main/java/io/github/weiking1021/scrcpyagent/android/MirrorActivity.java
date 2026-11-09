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
public final class MirrorActivity extends Activity implements SurfaceHolder.Callback {
    private ScrcpySessions.Entry entry;
    private SurfaceView video;
    private TextView status;
    private int videoWidth, videoHeight;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        try { entry = ScrcpySessions.get(getIntent().getStringExtra("sessionKey")); }
        catch (Exception error) { finishAndRemoveTask(); return; }
        entry.activity = this;
        setTitle(entry.request.optString("title"));
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        getWindow().setDecorFitsSystemWindows(false);
        var frame = new FrameLayout(this);
        frame.setBackgroundColor(android.graphics.Color.BLACK);
        frame.setOnApplyWindowInsetsListener((view, insets) -> {
            var bars = insets.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            view.setPadding(bars.left, bars.top, bars.right, bars.bottom);
            return WindowInsets.CONSUMED;
        });
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
        setContentView(frame);
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
        entry.session = new MirrorSession(this, holder.getSurface(), new MirrorSession.Listener() {
            public void started() {
                entry.ready.complete(new JSONObject());
                runOnUiThread(() -> status.setVisibility(View.GONE));
            }
            public void size(int width, int height) { runOnUiThread(() -> {
                videoWidth = width; videoHeight = height; fit((FrameLayout)video.getParent());
            }); }
            public void failed(Exception error) { entry.end(error); runOnUiThread(MirrorActivity.this::finishAndRemoveTask); }
        }, entry.request);
        new Thread(() -> {
            try {
                var address = DeviceBackend.endpoint(entry.request.getString("serial"));
                entry.session.run(address[0], Integer.parseInt(address[1]));
                entry.end(null);
            } catch (Exception error) { entry.end(error); }
            runOnUiThread(this::finishAndRemoveTask);
        }, "scrcpy-video").start();
    }

    @Override public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) { }
    @Override public void surfaceDestroyed(SurfaceHolder holder) { if (entry != null && entry.session != null) entry.session.setSurface(null); }
    @Override protected void onDestroy() {
        if (entry != null) {
            if (entry.session != null) entry.session.stop();
            entry.activity = null;
            entry.end(null);
        }
        super.onDestroy();
    }
}
