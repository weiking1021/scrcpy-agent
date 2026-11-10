package io.github.weiking1021.scrcpyagent.android;

import java.io.DataInputStream;
import java.io.IOException;

/** scrcpy 5 raw audio: 48 kHz stereo, signed 16-bit little-endian PCM, with 12-byte packet headers. */
final class PcmAudio {
    static final int SAMPLE_RATE = 48000;
    static final int FRAME_BYTES = 4;

    static boolean header(DataInputStream input) throws IOException {
        int codec = input.readInt();
        if (codec == 0) return false;
        if (codec == 1) throw new IOException("Remote audio configuration failed");
        if (codec != 0x00726177) throw new IOException("Remote audio is not PCM");
        return true;
    }

    static byte[] packet(DataInputStream input) throws IOException {
        long pts = input.readLong();
        int size = input.readInt();
        if ((pts & 0xc000000000000000L) != 0 || size <= 0 || size > 256 * 1024 || size % FRAME_BYTES != 0)
            throw new IOException("Invalid PCM audio packet");
        byte[] pcm = new byte[size];
        input.readFully(pcm);
        return pcm;
    }
}
