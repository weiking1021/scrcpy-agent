package io.github.weiking1021.scrcpyagent.android;

import java.io.IOException;
import org.junit.Test;
import static org.junit.Assert.*;

public final class ShareImportTest {
    @Test public void allFourAliasesMapToExistingRequestActions() {
        String prefix = ShareImport.class.getPackage().getName() + ".";
        assertEquals("send-choose", ShareImport.actionFor(prefix + "ShareSendChoose"));
        assertEquals("send-default", ShareImport.actionFor(prefix + "ShareSendDefault"));
        assertEquals("open-choose", ShareImport.actionFor(prefix + "ShareOpenChoose"));
        assertEquals("open-default", ShareImport.actionFor(prefix + "ShareOpenDefault"));
        assertNull(ShareImport.actionFor(prefix + "MainActivity"));
        assertNull(ShareImport.actionFor("other.app.ShareSendChoose"));
    }

    @Test public void filenamesStayInsideTheirImportFolderAndKeepTheirOriginalExtension() throws Exception {
        assertEquals("照片 01.jpg", ShareImport.checkedName("照片 01.jpg"));
        assertEquals("report.txt", ShareImport.checkedName("report.txt"));
        for (String name : new String[]{"", ".", "..", "../private", "/private", "a\\b", "a\nb", "a\0b", "a".repeat(256)}) {
            assertThrows(name, IOException.class, () -> ShareImport.checkedName(name));
        }
    }
}
