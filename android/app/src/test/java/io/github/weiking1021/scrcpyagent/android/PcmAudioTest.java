package io.github.weiking1021.scrcpyagent.android;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import org.junit.Test;
import static org.junit.Assert.*;

public class PcmAudioTest {
    private static DataInputStream stream(int codec, long pts, int length, byte[] payload) throws IOException {
        var bytes = new ByteArrayOutputStream();
        var output = new DataOutputStream(bytes);
        output.writeInt(codec); output.writeLong(pts); output.writeInt(length); output.write(payload);
        return new DataInputStream(new ByteArrayInputStream(bytes.toByteArray()));
    }
    @Test public void readsPcmWithoutChangingSampleBytes() throws Exception {
        byte[] pcm = {0, 1, -1, 127, 16, 32, 64, -128};
        var input = stream(0x00726177, 123456, pcm.length, pcm);
        assertTrue(PcmAudio.header(input));
        assertArrayEquals(pcm, PcmAudio.packet(input));
        assertEquals(-1, input.read());
    }
    @Test public void handlesUnavailableAudioWithoutTreatingItAsPcm() throws Exception {
        assertFalse(PcmAudio.header(stream(0, 0, 0, new byte[0])));
        for (int codec : new int[]{1, 0x6f707573}) {
            try { PcmAudio.header(stream(codec, 0, 0, new byte[0])); fail(); }
            catch (IOException expected) { }
        }
    }
    @Test public void rejectsInvalidAndTruncatedPackets() throws Exception {
        for (int size : new int[]{-1, 0, 3, 262148, Integer.MAX_VALUE, 8}) {
            var input = stream(0x00726177, 0, size, new byte[4]);
            assertTrue(PcmAudio.header(input));
            try { PcmAudio.packet(input); fail("size " + size); }
            catch (IOException expected) { }
        }
        var input = stream(0x00726177, Long.MIN_VALUE, 4, new byte[4]);
        assertTrue(PcmAudio.header(input));
        try { PcmAudio.packet(input); fail(); } catch (IOException expected) { }
    }
}
