package io.github.weiking1021.scrcpyagent.android;

import android.os.Bundle;

/** Shell launch entry, restricted to a live session's single-use random token. */
public final class ShizukuMirrorActivity extends MirrorActivity {
    @Override public void onCreate(Bundle state) {
        try {
            var entry = ScrcpySessions.get(getIntent().getStringExtra("sessionKey"));
            if (entry.mode != MirrorWindows.Mode.SHIZUKU
                || !entry.launchToken.equals(getIntent().getStringExtra("launchToken"))
                || (state == null && !entry.tokenUsed.compareAndSet(false, true)))
                throw new SecurityException("Invalid window launch");
        } catch (Exception error) {
            getIntent().removeExtra("sessionKey");
        }
        super.onCreate(state);
    }
}
