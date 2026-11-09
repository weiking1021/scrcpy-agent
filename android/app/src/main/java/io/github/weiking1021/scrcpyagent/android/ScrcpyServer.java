package io.github.weiking1021.scrcpyagent.android;

import android.content.Context;
import java.io.IOException;
import java.util.UUID;
import org.json.JSONObject;

/** Shared upload and launch path for mirroring and server queries. */
final class ScrcpyServer {
    static String command(Context context, AdbClient adb, JSONObject request, String connectionOptions) throws Exception {
        String version = request.getString("version");
        var server = new java.io.File(request.getString("serverPath")).getCanonicalFile();
        if (!server.toPath().startsWith(context.getDataDir().getCanonicalFile().toPath()))
            throw new SecurityException("Server must stay inside application storage");
        if (!server.isFile()) throw new IOException("The selected scrcpy server is not installed");
        String remote = "/data/local/tmp/scrcpy-agent-" + UUID.randomUUID().toString().replace("-", "") + ".jar";
        AdbSync.push(adb, server, remote, sent -> { });
        var options = request.getJSONArray("options");
        var arguments = new StringBuilder(connectionOptions);
        for (int index = 0; index < options.length(); index++) arguments.append(' ').append(options.getString(index));
        return "CLASSPATH=" + remote + " app_process / com.genymobile.scrcpy.Server " + version + " " + arguments;
    }

}
