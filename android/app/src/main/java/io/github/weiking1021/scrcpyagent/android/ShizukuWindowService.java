package io.github.weiking1021.scrcpyagent.android;

import androidx.annotation.Keep;
import java.util.concurrent.TimeUnit;

/** Runs as Shizuku's shell identity; accepts only mirror launch and window-size operations. */
@Keep
public final class ShizukuWindowService extends IWindowControl.Stub {
    public ShizukuWindowService() { }

    @Override public void destroy() { System.exit(0); }

    @Override public String launch(String key, String token) {
        if (key == null || key.isEmpty() || token == null || token.isEmpty()) return "Missing mirror session";
        return command("/system/bin/am", "start", "-W", "--windowingMode", "5", "-f", "0x18000000",
            "-n", BuildConfig.APPLICATION_ID + "/" + ShizukuMirrorActivity.class.getName(),
            "--es", "sessionKey", key, "--es", "launchToken", token);
    }

    @Override public String resize(int task, int left, int top, int right, int bottom) {
        if (task < 0 || right <= left || bottom <= top) return "Invalid mirror bounds";
        return command("/system/bin/am", "task", "resize", Integer.toString(task),
            Integer.toString(left), Integer.toString(top), Integer.toString(right), Integer.toString(bottom));
    }

    private static String command(String... arguments) {
        Process process = null;
        try {
            process = new ProcessBuilder(arguments).redirectErrorStream(true).start();
            if (!process.waitFor(15, TimeUnit.SECONDS)) return "Window operation timed out";
            String output;
            try (var reader = new java.io.BufferedReader(new java.io.InputStreamReader(process.getInputStream(), java.nio.charset.StandardCharsets.UTF_8))) {
                output = reader.lines().collect(java.util.stream.Collectors.joining("\n"));
            }
            if (process.exitValue() != 0 || output.contains("Error:") || output.contains("Exception"))
                return "The system refused the window operation";
            return null;
        } catch (Exception error) {
            if (error instanceof InterruptedException) Thread.currentThread().interrupt();
            return "Window operation failed: " + error.getClass().getSimpleName();
        } finally {
            if (process != null) process.destroy();
        }
    }
}
