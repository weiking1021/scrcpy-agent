package io.github.weiking1021.scrcpyagent.android;

import java.io.IOException;
import java.nio.charset.StandardCharsets;

/** Share targets and file-name validation, independent of Android URI grants. */
final class ShareImport {
    static String actionFor(String component) {
        String prefix = ShareImport.class.getPackage().getName() + ".";
        if (component.equals(prefix + "ShareSendChoose")) return "send-choose";
        if (component.equals(prefix + "ShareSendDefault")) return "send-default";
        if (component.equals(prefix + "ShareOpenChoose")) return "open-choose";
        if (component.equals(prefix + "ShareOpenDefault")) return "open-default";
        return null;
    }

    static String checkedName(String name) throws IOException {
        if (name.isEmpty() || name.equals(".") || name.equals("..") || name.contains("/") || name.contains("\\")
            || name.getBytes(StandardCharsets.UTF_8).length > 255 || name.codePoints().anyMatch(Character::isISOControl))
            throw new IOException("Invalid shared file name");
        return name;
    }
}
