package io.github.weiking1021.scrcpyagent.android;

/** Proportional video sizing shared by the native window presentations. */
final class MirrorGeometry {
    static int fitWidth(int maxWidth, int maxHeight, int videoWidth, int videoHeight, int extraHeight) {
        return Math.max(1, Math.min(maxWidth,
            (int)((long)Math.max(1, maxHeight - extraHeight) * videoWidth / videoHeight)));
    }

    static int heightForWidth(int width, int videoWidth, int videoHeight, int extraHeight) {
        return Math.max(1, (int)Math.round((double)width * videoHeight / videoWidth)) + extraHeight;
    }
}
