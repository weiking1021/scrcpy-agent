package io.github.weiking1021.scrcpyagent.android;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.EOFException;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import org.junit.Test;
import static org.junit.Assert.*;

public final class AdbSyncTest {
    private static final byte[] OKAY = {'O', 'K', 'A', 'Y', 0, 0, 0, 0};

    private static int header(DataInputStream input, String expected) throws IOException {
        byte[] id = new byte[4];
        input.readFully(id);
        assertEquals(expected, new String(id, StandardCharsets.US_ASCII));
        return Integer.reverseBytes(input.readInt());
    }

    @Test public void sendsUnchangedBytesInBoundedChunksWithProgress() throws Exception {
        byte[] content = new byte[150000];
        for (int index = 0; index < content.length; index++) content[index] = (byte)index;
        var wire = new ByteArrayOutputStream();
        var progress = new ArrayList<Long>();
        assertEquals(content.length, AdbSync.send(new ByteArrayInputStream(content),
            new ByteArrayInputStream(OKAY), wire, "/sdcard/Download/test.bin", 42, progress::add));
        var input = new DataInputStream(new ByteArrayInputStream(wire.toByteArray()));
        byte[] destination = new byte[header(input, "SEND")];
        input.readFully(destination);
        assertEquals("/sdcard/Download/test.bin,33188", new String(destination, StandardCharsets.UTF_8));
        var received = new ByteArrayOutputStream();
        for (int expected : new int[]{65536, 65536, 18928}) {
            assertEquals(expected, header(input, "DATA"));
            byte[] chunk = new byte[expected];
            input.readFully(chunk);
            received.write(chunk);
        }
        assertArrayEquals(content, received.toByteArray());
        assertEquals(42, header(input, "DONE"));
        assertEquals(0, header(input, "QUIT"));
        assertEquals(-1, input.read());
        assertEquals(List.of(65536L, 131072L, 150000L), progress);
    }

    @Test public void emptyFilesStillSendAndFinishWithoutDataPackets() throws Exception {
        var wire = new ByteArrayOutputStream();
        var progress = new ArrayList<Long>();
        assertEquals(0, AdbSync.send(new ByteArrayInputStream(new byte[0]),
            new ByteArrayInputStream(OKAY), wire, "/sdcard/empty", 0, progress::add));
        var input = new DataInputStream(new ByteArrayInputStream(wire.toByteArray()));
        input.skipBytes(header(input, "SEND"));
        assertEquals(0, header(input, "DONE"));
        assertEquals(0, header(input, "QUIT"));
        assertTrue(progress.isEmpty());
    }

    @Test public void deviceFailureIsNotReportedAsSuccess() {
        byte[] error = {'F', 'A', 'I', 'L', 8, 0, 0, 0, 'n', 'o', ' ', 's', 'p', 'a', 'c', 'e'};
        IOException failure = assertThrows(IOException.class, () -> AdbSync.send(
            new ByteArrayInputStream(new byte[]{1}), new ByteArrayInputStream(error),
            new ByteArrayOutputStream(), "/sdcard/test", 0, sent -> { }));
        assertEquals("no space", failure.getMessage());
    }

    @Test public void rejectsTruncatedRepliesAndInvalidPaths() {
        assertThrows(EOFException.class, () -> AdbSync.send(new ByteArrayInputStream(new byte[0]),
            new ByteArrayInputStream(new byte[]{'O', 'K'}), new ByteArrayOutputStream(), "/sdcard/test", 0, sent -> { }));
        var wire = new ByteArrayOutputStream();
        assertThrows(IOException.class, () -> AdbSync.send(new ByteArrayInputStream(new byte[0]),
            new ByteArrayInputStream(OKAY), wire, "/sdcard/a\nb", 0, sent -> { }));
        assertEquals(0, wire.size());
    }
}
