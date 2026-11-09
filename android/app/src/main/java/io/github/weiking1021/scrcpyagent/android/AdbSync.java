package io.github.weiking1021.scrcpyagent.android;

import java.io.DataInputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.function.LongConsumer;

/** The sync service used by both shared file transfers and the scrcpy server upload. */
final class AdbSync {
    static long push(AdbClient client, File local, String remote, LongConsumer progress) throws Exception {
        if (!local.isFile()) throw new IOException("Source is not a file");
        // Open the source before asking the device for anything.
        try (var source = new FileInputStream(local); var stream = client.openStream("sync:");
             var input = stream.openInputStream(); var output = stream.openOutputStream()) {
            return send(source, input, output, remote, local.lastModified() / 1000, progress);
        }
    }

    static long send(InputStream source, InputStream response, OutputStream output,
        String remote, long modified, LongConsumer progress) throws IOException {
        if (remote.isEmpty() || remote.indexOf('\0') >= 0 || remote.indexOf('\n') >= 0)
            throw new IOException("Invalid remote path");
        byte[] destination = (remote + ",33188").getBytes(StandardCharsets.UTF_8);
        if (destination.length > 1024) throw new IOException("Remote path exceeds the sync protocol limit");
        write(output, "SEND", destination, destination.length);
        byte[] buffer = new byte[64 * 1024];
        long sent = 0;
        int count;
        while ((count = source.read(buffer)) != -1) {
            if (count == 0) continue;
            write(output, "DATA", buffer, count);
            sent += count;
            progress.accept(sent);
        }
        header(output, "DONE", (int)Math.min(0xffffffffL, Math.max(0, modified)));
        output.flush();
        var input = new DataInputStream(response);
        byte[] id = new byte[4];
        input.readFully(id);
        int length = Integer.reverseBytes(input.readInt());
        String status = new String(id, StandardCharsets.US_ASCII);
        if (status.equals("FAIL")) {
            if (length < 0 || length > 64 * 1024) throw new IOException("Invalid sync failure message length");
            byte[] error = new byte[length];
            input.readFully(error);
            throw new IOException(new String(error, StandardCharsets.UTF_8));
        }
        if (!status.equals("OKAY") || length != 0) throw new IOException("Invalid sync response");
        // The transfer has succeeded; an error saying goodbye cannot change that verdict.
        try { header(output, "QUIT", 0); output.flush(); } catch (IOException ignored) { }
        return sent;
    }

    private static void write(OutputStream output, String id, byte[] bytes, int length) throws IOException {
        header(output, id, length);
        output.write(bytes, 0, length);
    }

    private static void header(OutputStream output, String id, int value) throws IOException {
        output.write(id.getBytes(StandardCharsets.US_ASCII));
        output.write(ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(value).array());
    }
}
