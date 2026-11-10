package io.github.weiking1021.scrcpyagent.android;

import android.content.Context;
import android.media.MediaCodec;
import android.media.MediaFormat;
import android.view.Surface;
import io.github.muntashirakon.adb.AdbStream;
import java.io.DataInputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.Locale;

/** Upload the matching scrcpy server and decode its v5 H.264 stream on a Surface. */
final class MirrorSession {
    interface Listener {
        void started();
        void size(int width, int height);
        void failed(Exception error);
    }

    private final Context context;
    private volatile Surface surface;
    private final Listener listener;
    private final Object codecLock = new Object();
    private volatile boolean stopped;
    private volatile AdbClient adb;
    private volatile AdbStream video;
    private volatile AdbStream shell;
    private volatile AdbStream control;
    private volatile OutputStream controlOutput;
    private final java.util.concurrent.ExecutorService inputEvents = new java.util.concurrent.ThreadPoolExecutor(
        1, 1, 0L, java.util.concurrent.TimeUnit.MILLISECONDS,
        new java.util.concurrent.ArrayBlockingQueue<>(256),
        task -> { var thread = new Thread(task, "scrcpy-input"); thread.setDaemon(true); return thread; });
    private MediaCodec decoder;
    private final org.json.JSONObject request;
    private boolean started;
    private volatile DeviceAudio.Lease audio;

    MirrorSession(Context context, Surface surface, Listener listener, org.json.JSONObject request) {
        this.context = context.getApplicationContext();
        this.surface = surface;
        this.listener = listener;
        this.request = request;
    }

    void run(String host, int port) {
        try {
            adb = new AdbClient(context);
            if (stopped) return;
            if (!adb.connect(host, port)) throw new IOException("ADB connection was not accepted");
            if (stopped) return;
            audio = DeviceAudio.acquire(context, request);
            if (stopped) return;
            int id = new java.security.SecureRandom().nextInt(Integer.MAX_VALUE);
            String socket = String.format(Locale.ROOT, "scrcpy_%08x", id);
            String command = ScrcpyServer.command(context, adb, request, "scid=" + Integer.toHexString(id));
            if (stopped) return;
            shell = adb.openStream("shell:" + command);
            var logs = new Thread(() -> {
                try (var input = shell.openInputStream()) {
                    byte[] bytes = new byte[2048];
                    while (!stopped && input.read(bytes) != -1) { /* Drain server output, never log device data. */ }
                } catch (Exception ignored) { }
            }, "scrcpy-server-output");
            logs.setDaemon(true);
            logs.start();
            long deadline = System.nanoTime() + 15_000_000_000L;
            while (!stopped && video == null) {
                try { video = adb.openStream("localabstract:" + socket); }
                catch (IOException error) {
                    if (System.nanoTime() >= deadline) throw new IOException("scrcpy server did not start", error);
                    Thread.sleep(100);
                }
            }
            if (stopped) return;
            // The server accepts video, then control, before it sends stream metadata.
            control = adb.openStream("localabstract:" + socket);
            controlOutput = control.openOutputStream();
            var messages = new Thread(() -> {
                try (var input = control.openInputStream()) {
                    byte[] bytes = new byte[2048];
                    while (!stopped && input.read(bytes) != -1) { /* Drain device messages. */ }
                } catch (Exception error) {
                    if (!stopped) { listener.failed(error); stop(); }
                }
            }, "scrcpy-control-output");
            messages.setDaemon(true);
            messages.start();
            try (var input = new DataInputStream(video.openInputStream())) {
                if (input.readUnsignedByte() != 0) throw new IOException("Invalid scrcpy prelude");
                input.readFully(new byte[64]); // Device name; the shared core supplies the window title.
                if (input.readInt() != 0x68323634) throw new IOException("The stream is not H.264");
                var output = new Thread(this::drain, "scrcpy-video-output");
                output.setDaemon(true);
                output.start();
                // Same v5 framing as src-tauri/src/core/scrcpy/v5.rs.
                byte[] header = new byte[12];
                while (!stopped) {
                    input.readFully(header);
                    var fields = ByteBuffer.wrap(header).order(ByteOrder.BIG_ENDIAN);
                    long flags = fields.getLong();
                    int size = fields.getInt();
                    if ((flags & Long.MIN_VALUE) != 0) {
                        configure(fields.getInt(4), size);
                        if (!started && surface != null && surface.isValid()) {
                            var initial = request.optJSONArray("initialControl");
                            if (initial != null) {
                                byte[] packet = new byte[initial.length()];
                                for (int index = 0; index < packet.length; index++) packet[index] = (byte)initial.getInt(index);
                                sendControl(packet);
                            }
                            started = true;
                            listener.started();
                        }
                        continue;
                    }
                    if (size <= 0 || size > 8 * 1024 * 1024) throw new IOException("Invalid video packet size");
                    byte[] packet = new byte[size];
                    input.readFully(packet);
                    feed(packet, flags & 0x1fffffffffffffffL, (flags & (1L << 62)) != 0);
                }
            }
        } catch (Exception error) {
            if (!stopped) listener.failed(error);
        } finally {
            stopped = true;
            close(audio);
            inputEvents.shutdownNow();
            controlOutput = null;
            close(control);
            close(video);
            close(shell);
            close(adb);
            synchronized (codecLock) { releaseDecoder(); }
        }
    }

    void stop() {
        stopped = true;
        close(audio);
        inputEvents.shutdownNow();
        // Closing the transport interrupts blocking reads/open operations off the UI thread.
        var cleanup = new Thread(() -> close(adb), "scrcpy-disconnect");
        cleanup.setDaemon(true);
        cleanup.start();
    }

    boolean hasControl() { return !stopped && controlOutput != null; }

    void setSurface(Surface surface) {
        synchronized (codecLock) {
            releaseDecoder();
            this.surface = surface;
        }
    }

    void sendControl(byte[] packet) throws IOException {
        if (stopped || controlOutput == null) throw new IOException("Control channel is closed");
        try {
            inputEvents.execute(() -> {
                try {
                    var output = controlOutput;
                    if (stopped || output == null) return;
                    output.write(packet);
                    output.flush();
                } catch (IOException error) {
                    if (!stopped) { listener.failed(error); stop(); }
                }
            });
        } catch (java.util.concurrent.RejectedExecutionException error) {
            throw new IOException("Control queue is unavailable", error);
        }
    }

    private void configure(int width, int height) throws IOException {
        if (width <= 0 || height <= 0 || width > 16384 || height > 16384) throw new IOException("Invalid video dimensions");
        synchronized (codecLock) {
            releaseDecoder();
            if (surface == null || !surface.isValid()) return;
            decoder = MediaCodec.createDecoderByType("video/avc");
            var format = MediaFormat.createVideoFormat("video/avc", width, height);
            format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 4 * 1024 * 1024);
            decoder.configure(format, surface, null, 0);
            decoder.start();
        }
        listener.size(width, height);
    }

    private void feed(byte[] packet, long pts, boolean config) throws IOException {
        while (!stopped) {
            synchronized (codecLock) {
                if (decoder == null) return; // Surface detached; wait for the new video session after reattachment.
                int index = decoder.dequeueInputBuffer(10000);
                if (index >= 0) {
                    var buffer = decoder.getInputBuffer(index);
                    if (buffer == null || buffer.capacity() < packet.length) throw new IOException("Video packet exceeds decoder input capacity");
                    buffer.clear();
                    buffer.put(packet);
                    decoder.queueInputBuffer(index, 0, packet.length, pts, config ? MediaCodec.BUFFER_FLAG_CODEC_CONFIG : 0);
                    return;
                }
            }
            Thread.yield();
        }
    }

    private void drain() {
        var info = new MediaCodec.BufferInfo();
        try {
            while (!stopped) {
                synchronized (codecLock) {
                    if (decoder != null) {
                        int index = decoder.dequeueOutputBuffer(info, 0);
                        if (index >= 0) decoder.releaseOutputBuffer(index, true);
                    }
                }
                Thread.sleep(2);
            }
        } catch (Exception error) {
            if (!stopped) { listener.failed(error); stop(); }
        }
    }

    private void releaseDecoder() {
        if (decoder == null) return;
        try { decoder.stop(); } catch (IllegalStateException ignored) { }
        decoder.release();
        decoder = null;
    }
    private static void close(AutoCloseable resource) {
        if (resource != null) try { resource.close(); } catch (Exception ignored) { }
    }
}
