package io.github.weiking1021.scrcpyagent.android;

import org.junit.Test;
import static org.junit.Assert.*;

public class MirrorGeometryTest {
    @Test public void resizingKeepsPaddingFixedAndImageProportional() {
        for (int[] stream : new int[][]{{1080, 2520}, {2520, 1080}, {1080, 1920}, {1920, 1080}}) {
            for (int padding : new int[]{0, 54, 288}) {
                int extraHeight = padding * 2 + 140;
                for (int requestedWidth : new int[]{300, 468, 960, 1600, 2560}) {
                    int width = MirrorGeometry.fitWidth(requestedWidth, 1600, stream[0], stream[1], extraHeight);
                    int height = MirrorGeometry.heightForWidth(width, stream[0], stream[1], extraHeight);
                    assertTrue(width <= requestedWidth);
                    assertTrue(height <= 1600);
                    assertEquals((double)width * stream[1] / stream[0], height - extraHeight, 0.5);
                    double scale = Math.min((double)width / stream[0], (double)(height - extraHeight) / stream[1]);
                    assertTrue("No visible side gap", width - Math.round(stream[0] * scale) <= 1);
                }
            }
        }
    }
}
