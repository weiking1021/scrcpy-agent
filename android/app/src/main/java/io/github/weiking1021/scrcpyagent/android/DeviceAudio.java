package io.github.weiking1021.scrcpyagent.android;

import android.content.Context;
import android.content.Intent;
import android.media.AudioAttributes;
import android.media.AudioFormat;
import android.media.AudioTrack;
import io.github.muntashirakon.adb.AdbStream;
import java.io.DataInputStream;
import java.io.IOException;
import java.util.HashMap;
import java.util.Locale;
import java.util.concurrent.atomic.AtomicBoolean;
import org.json.JSONObject;

/** One audio capture and player per remote device, independent of any individual window's lifetime. */
final class DeviceAudio {
    private static final HashMap<String, Source> sources = new HashMap<>();
    private static float volume;
    private static volatile String focus = "starting service";

    static synchronized Lease acquire(Context context, JSONObject request) {
        String device = request.optString("audioDevice", request.optString("serial"));
        var source = sources.get(device);
        if (source == null) {
            if (sources.isEmpty()) { volume = 0; focus = "starting service"; }
            source = new Source(context.getApplicationContext(), request, device);
            sources.put(device, source);
            try {
                context.startForegroundService(new Intent(context, AudioPlaybackService.class));
                source.thread.start();
            } catch (RuntimeException error) { android.util.Log.w("ScrcpyAgent", "Could not start mirror audio service", error); }
        } else {
            try { context.startForegroundService(new Intent(context, AudioPlaybackService.class)); }
            catch (RuntimeException error) { android.util.Log.w("ScrcpyAgent", "Could not start mirror audio service", error); }
        }
        source.users++;
        return new Lease(source);
    }

    static AudioAttributes attributes() {
        return new AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE).build();
    }

    static synchronized boolean active() { return !sources.isEmpty(); }

    static synchronized void focus(float level, String state) {
        volume = level; focus = state;
        for (var source : sources.values()) source.volume(level);
        DeviceAudio.class.notifyAll();
    }

    static final class Lease implements AutoCloseable {
        private final Source source;
        private final AtomicBoolean released = new AtomicBoolean();
        Lease(Source source) { this.source = source; }
        public void close() {
            if (!released.compareAndSet(false, true)) return;
            synchronized (DeviceAudio.class) {
                if (--source.users != 0) return;
                sources.remove(source.device, source);
                source.stop();
                if (sources.isEmpty()) source.context.stopService(new Intent(source.context, AudioPlaybackService.class));
            }
        }
    }

    private static final class Source {
        final Context context;
        final JSONObject request;
        final String device;
        final Thread thread;
        final Object trackLock = new Object();
        volatile int users;
        volatile boolean stopped;
        volatile AdbClient adb;
        AudioTrack track;

        Source(Context context, JSONObject request, String device) {
            this.context = context; this.request = request; this.device = device;
            thread = new Thread(this::run, "scrcpy-audio");
            thread.setDaemon(true);
        }

        void run() {
            try {
                synchronized (DeviceAudio.class) {
                    long deadline = System.nanoTime() + 15_000_000_000L;
                    while (!stopped && volume == 0 && System.nanoTime() < deadline) DeviceAudio.class.wait(250);
                    if (stopped) return;
                    if (volume == 0) throw new IOException("Local playback is not allowed: " + focus);
                }
                var address = DeviceBackend.endpoint(request.getString("serial"));
                adb = new AdbClient(context);
                if (stopped) return;
                if (!adb.connect(address[0], Integer.parseInt(address[1]))) throw new IOException("Audio ADB connection was refused");
                if (stopped) return;
                var audioRequest = new JSONObject(request.toString()).put("options", request.getJSONArray("audioOptions"));
                int id = new java.security.SecureRandom().nextInt(Integer.MAX_VALUE);
                String socket = String.format(Locale.ROOT, "scrcpy_%08x", id);
                String command = ScrcpyServer.command(context, adb, audioRequest, "scid=" + Integer.toHexString(id));
                if (stopped) return;
                try (var shell = adb.openStream("shell:" + command)) {
                    var drain = new Thread(() -> {
                        try (var input = shell.openInputStream()) {
                            byte[] bytes = new byte[2048];
                            while (!stopped && input.read(bytes) != -1) { /* Do not expose remote server output. */ }
                        } catch (IOException ignored) { }
                    }, "scrcpy-audio-server");
                    drain.setDaemon(true); drain.start();
                    try (var stream = connect(socket); var input = new DataInputStream(stream.openInputStream())) {
                        if (!PcmAudio.header(input)) return;
                        if (stopped) return;
                        int minimum = AudioTrack.getMinBufferSize(PcmAudio.SAMPLE_RATE, AudioFormat.CHANNEL_OUT_STEREO, AudioFormat.ENCODING_PCM_16BIT);
                        if (minimum <= 0) throw new IOException("PCM audio format is not supported");
                        var player = new AudioTrack.Builder().setAudioAttributes(attributes())
                            .setAudioFormat(new AudioFormat.Builder().setSampleRate(PcmAudio.SAMPLE_RATE)
                                .setEncoding(AudioFormat.ENCODING_PCM_16BIT).setChannelMask(AudioFormat.CHANNEL_OUT_STEREO).build())
                            .setTransferMode(AudioTrack.MODE_STREAM).setBufferSizeInBytes(Math.max(minimum, 9600))
                            .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY).build();
                        synchronized (DeviceAudio.class) {
                            synchronized (trackLock) {
                                track = player;
                                if (stopped) return;
                                player.setVolume(volume); player.play();
                            }
                        }
                        while (!stopped) {
                            byte[] pcm = PcmAudio.packet(input);
                            int offset = 0;
                            while (!stopped && offset < pcm.length) {
                                int count = player.write(pcm, offset, pcm.length - offset, AudioTrack.WRITE_BLOCKING);
                                if (count <= 0) throw new IOException("Audio playback failed: " + count);
                                offset += count;
                            }
                        }
                    }
                }
            } catch (Exception error) {
                if (!stopped) android.util.Log.w("ScrcpyAgent", "Mirror audio playback stopped", error);
            } finally {
                synchronized (trackLock) {
                    if (track != null) {
                        try { track.stop(); } catch (IllegalStateException ignored) { }
                        track.release(); track = null;
                    }
                }
                close(adb);
            }
        }

        private AdbStream connect(String socket) throws Exception {
            long deadline = System.nanoTime() + 15_000_000_000L;
            while (!stopped) {
                try { return adb.openStream("localabstract:" + socket); }
                catch (IOException error) {
                    if (System.nanoTime() >= deadline) throw new IOException("Audio server did not start", error);
                    Thread.sleep(100);
                }
            }
            throw new IOException("Audio closed");
        }

        void volume(float level) {
            synchronized (trackLock) {
                if (track != null) track.setVolume(level);
            }
        }

        void stop() {
            if (stopped) return;
            stopped = true;
            synchronized (trackLock) {
                if (track != null) try { track.stop(); } catch (IllegalStateException ignored) { }
            }
            var cleanup = new Thread(() -> close(adb), "scrcpy-audio-close");
            cleanup.setDaemon(true); cleanup.start();
        }
    }

    private static void close(AutoCloseable resource) {
        if (resource != null) try { resource.close(); } catch (Exception ignored) { }
    }
}
