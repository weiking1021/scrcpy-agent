package io.github.weiking1021.scrcpyagent.android;

import android.app.Activity;
import android.content.Context;
import android.net.nsd.NsdManager;
import android.net.nsd.NsdServiceInfo;
import android.net.wifi.WifiManager;
import io.github.muntashirakon.adb.AdbStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.net.Inet4Address;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.concurrent.ConcurrentHashMap;
import org.json.JSONArray;
import org.json.JSONObject;

/** Android transport and NSD only. Discovery policy and persistence belong to Rust. */
final class DeviceBackend implements AutoCloseable {
    private final Activity activity;
    private final java.util.function.BiConsumer<String, JSONObject> events;
    private final NsdManager nsd;
    private final WifiManager.MulticastLock multicast;
    private final ConcurrentHashMap<String, AdbClient> clients = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<String, String> services = new ConcurrentHashMap<>();
    private final java.util.Set<String> present = ConcurrentHashMap.newKeySet();
    private final ArrayList<NsdManager.DiscoveryListener> listeners = new ArrayList<>();
    private final ArrayDeque<NsdServiceInfo> pending = new ArrayDeque<>();
    private boolean resolving;
    private volatile boolean closed;

    private static String serviceKey(NsdServiceInfo info) { return info.getServiceName() + "|" + info.getServiceType(); }

    DeviceBackend(Activity activity, java.util.function.BiConsumer<String, JSONObject> events) {
        this.activity = activity;
        this.events = events;
        nsd = (NsdManager) activity.getSystemService(Context.NSD_SERVICE);
        var wifi = (WifiManager) activity.getApplicationContext().getSystemService(Context.WIFI_SERVICE);
        multicast = wifi.createMulticastLock("scrcpy-agent-discovery");
        multicast.setReferenceCounted(false);
        multicast.acquire();
        for (String type : new String[]{"_adb-tls-connect._tcp.", "_adb-tls-pairing._tcp.", "_adb._tcp."}) {
            var listener = new NsdManager.DiscoveryListener() {
                public void onDiscoveryStarted(String type) { }
                public void onDiscoveryStopped(String type) { }
                public void onStartDiscoveryFailed(String type, int error) { }
                public void onStopDiscoveryFailed(String type, int error) { }
                public void onServiceFound(NsdServiceInfo info) {
                    activity.runOnUiThread(() -> {
                        if (!closed) { present.add(serviceKey(info)); pending.add(info); resolveNext(); }
                    });
                }
                public void onServiceLost(NsdServiceInfo info) {
                    present.remove(serviceKey(info)); services.remove(serviceKey(info));
                }
            };
            listeners.add(listener);
            nsd.discoverServices(type, NsdManager.PROTOCOL_DNS_SD, listener);
        }
    }

    // Older Android releases permit only one resolve request at a time.
    private void resolveNext() {
        if (closed || resolving || pending.isEmpty()) return;
        resolving = true;
        var info = pending.remove();
        nsd.resolveService(info, new NsdManager.ResolveListener() {
            private void done() { activity.runOnUiThread(() -> { resolving = false; resolveNext(); }); }
            public void onResolveFailed(NsdServiceInfo service, int error) { done(); }
            public void onServiceResolved(NsdServiceInfo service) {
                var address = service.getHost();
                if (android.os.Build.VERSION.SDK_INT >= 34) {
                    address = service.getHostAddresses().stream().filter(a -> a instanceof Inet4Address).findFirst().orElse(null);
                }
                if (!closed && present.contains(serviceKey(info)) && address instanceof Inet4Address) {
                    services.put(serviceKey(info),
                        // Resolved types have a leading dot on Android; keep the discovered type.
                        service.getServiceName() + "\t" + info.getServiceType() + "\t"
                            + address.getHostAddress() + ":" + service.getPort());
                }
                done();
            }
        });
    }

    Object execute(String operation, JSONObject data) throws Exception {
        switch (operation) {
            case "bundledServer": {
                var destination = new java.io.File(data.getString("destination")).getCanonicalFile();
                if (!destination.toPath().startsWith(activity.getDataDir().getCanonicalFile().toPath()))
                    throw new SecurityException("Server asset must stay inside application storage");
                var parent = destination.getParentFile();
                if (parent != null && !parent.isDirectory() && !parent.mkdirs()) throw new java.io.IOException("Could not create server folder");
                try (var source = activity.getAssets().open("scrcpy-server.jar"); var output = new java.io.FileOutputStream(destination)) {
                    byte[] bytes = new byte[64 * 1024];
                    int count;
                    while ((count = source.read(bytes)) != -1) output.write(bytes, 0, count);
                }
                return BuildConfig.SCRCPY_VERSION;
            }
            case "mdns": return String.join("\n", services.values());
            case "devices": {
                var result = new StringBuilder();
                for (var entry : clients.entrySet()) {
                    if (entry.getValue().isConnected()) result.append(entry.getKey()).append("\tdevice\n");
                    else if (clients.remove(entry.getKey(), entry.getValue())) entry.getValue().close();
                }
                return result.toString();
            }
            case "pair": {
                var address = endpoint(data.getString("address"));
                try (var client = new AdbClient(activity)) {
                    if (!client.pair(address[0], Integer.parseInt(address[1]), data.getString("code")))
                        throw new java.io.IOException("Pairing was not accepted");
                }
                return "paired";
            }
            case "connect": {
                String target = data.getString("address");
                var existing = clients.get(target);
                if (existing != null && existing.isConnected()) return "connected";
                var address = endpoint(target);
                var client = new AdbClient(activity);
                try {
                    if (!client.connect(address[0], Integer.parseInt(address[1]))) throw new java.io.IOException("ADB connection was not accepted");
                    var previous = clients.put(target, client);
                    if (previous != null) previous.close();
                } catch (Exception error) { client.close(); throw error; }
                return "connected";
            }
            case "disconnect": {
                var client = clients.remove(data.getString("address"));
                if (client != null) client.close();
                return "disconnected";
            }
            case "shell": case "queryServer": {
                var client = clients.get(data.getString("serial"));
                if (client == null) throw new java.io.IOException("Device is not connected");
                String command = operation.equals("queryServer")
                    ? ScrcpyServer.command(activity, client, data, "")
                    : data.getString("command");
                return shell(client, command);
            }
            case "push": {
                var client = clients.get(data.getString("serial"));
                if (client == null) throw new java.io.IOException("Device is not connected");
                long id = data.getLong("id");
                return client.push(new java.io.File(data.getString("local")), data.getString("remote"), sent -> {
                    try { events.accept("adb-push", new JSONObject().put("id", id).put("sent", sent)); }
                    catch (org.json.JSONException error) { throw new IllegalStateException(error); }
                });
            }
            case "startSession": return ScrcpySessions.start(activity, data, input -> events.accept("touch", input));
            case "waitSession": return ScrcpySessions.waitFor(data.getString("key"));
            case "cancelSession": ScrcpySessions.cancel(data.getString("key")); return new JSONObject();
            case "sendControl": {
                var session = ScrcpySessions.get(data.getString("key")).session;
                if (session == null) throw new java.io.IOException("Session is not ready");
                var values = data.getJSONArray("packet");
                byte[] packet = new byte[values.length()];
                for (int index = 0; index < packet.length; index++) packet[index] = (byte)values.getInt(index);
                session.sendControl(packet);
                return new JSONObject();
            }
            default: throw new UnsupportedOperationException(operation);
        }
    }

    static String[] endpoint(String value) {
        int split = value.lastIndexOf(':');
        if (split <= 0) throw new IllegalArgumentException("Invalid device address");
        return new String[]{value.substring(0, split), value.substring(split + 1)};
    }

    private JSONObject shell(AdbClient client, String command) throws Exception {
        try (AdbStream stream = client.openStream("shell,v2,raw:" + command);
             var input = new DataInputStream(stream.openInputStream())) {
            var stdout = new ByteArrayOutputStream();
            var stderr = new ByteArrayOutputStream();
            while (true) {
                int channel = input.readUnsignedByte();
                int size = Integer.reverseBytes(input.readInt());
                if (size < 0 || size > 16 * 1024 * 1024 || stdout.size() + stderr.size() + size > 16 * 1024 * 1024)
                    throw new java.io.IOException("Shell output exceeds the limit");
                byte[] bytes = new byte[size];
                input.readFully(bytes);
                if (channel == 1) stdout.write(bytes);
                else if (channel == 2) stderr.write(bytes);
                else if (channel == 3 && size > 0) {
                    return new JSONObject().put("stdout", array(stdout.toByteArray()))
                        .put("stderr", array(stderr.toByteArray())).put("exit", bytes[0] & 255);
                }
            }
        }
    }
    private static JSONArray array(byte[] bytes) {
        var result = new JSONArray();
        for (byte value : bytes) result.put(value & 255);
        return result;
    }

    @Override public void close() {
        closed = true;
        ScrcpySessions.close();
        for (var listener : listeners) try { nsd.stopServiceDiscovery(listener); } catch (Exception ignored) { }
        if (multicast.isHeld()) multicast.release();
        for (var client : clients.values()) try { client.close(); } catch (Exception ignored) { }
        clients.clear();
        services.clear();
        present.clear();
    }
}
