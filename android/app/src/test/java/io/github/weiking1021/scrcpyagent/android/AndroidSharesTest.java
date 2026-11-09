package io.github.weiking1021.scrcpyagent.android;

import java.io.IOException;
import org.junit.Test;
import static org.junit.Assert.*;

public final class AndroidSharesTest {
    @Test public void allFourAliasesMapToExistingRequestActions() {
        String prefix = AndroidShares.class.getPackage().getName() + ".";
        assertEquals("send-choose", AndroidShares.actionFor(prefix + "ShareSendChoose"));
        assertEquals("send-default", AndroidShares.actionFor(prefix + "ShareSendDefault"));
        assertEquals("open-choose", AndroidShares.actionFor(prefix + "ShareOpenChoose"));
        assertEquals("open-default", AndroidShares.actionFor(prefix + "ShareOpenDefault"));
        assertNull(AndroidShares.actionFor(prefix + "MainActivity"));
        assertNull(AndroidShares.actionFor("other.app.ShareSendChoose"));
    }

    @Test public void filenamesStayInsideTheirImportFolderAndKeepTheirOriginalExtension() throws Exception {
        assertEquals("照片 01.jpg", AndroidShares.checkedName("照片 01.jpg"));
        assertEquals("report.txt", AndroidShares.checkedName("report.txt"));
        for (String name : new String[]{"", ".", "..", "../private", "/private", "a\\b", "a\nb", "a\0b", "a".repeat(256)}) {
            assertThrows(name, IOException.class, () -> AndroidShares.checkedName(name));
        }
    }
}
