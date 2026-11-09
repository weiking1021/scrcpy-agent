package io.github.weiking1021.scrcpyagent.android

import android.app.Activity
import android.content.Intent
import android.webkit.WebView
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin
import app.tauri.plugin.JSObject
import java.util.concurrent.CompletableFuture
import java.util.concurrent.Executors
import androidx.appcompat.app.AppCompatActivity

@TauriPlugin
class DevicePlugin(private val activity: Activity) : Plugin(activity) {
    private val workers = Executors.newFixedThreadPool(4)
    private val backend by lazy { DeviceBackend(activity) { event, data -> trigger(event, JSObject(data.toString())) } }
    private val shares by lazy { AndroidShares(activity) { trigger("share", JSObject()) } }

    private external fun initializeTls(context: android.content.Context)

    override fun load(webView: WebView) {
        initializeTls(activity.applicationContext)
        backend
        shares.receive(activity.intent)
    }

    override fun onNewIntent(intent: Intent) { shares.receive(intent) }

    override fun onDestroy(activity: AppCompatActivity) {
        workers.execute { backend.close(); shares.close() }
        workers.shutdown()
    }

    @Command
    fun execute(invoke: Invoke) {
        val args = invoke.getArgs()
        workers.execute {
            try {
                val data = args.getJSONObject("data")
                val result = when (val operation = args.getString("operation")) {
                    "takeShares" -> shares.take()
                    "releaseShare" -> { shares.release(data.getString("batch")); JSObject() }
                    else -> backend.execute(operation, data)
                }
                if (result is CompletableFuture<*>) result.whenComplete { value, error ->
                    if (error != null) invoke.reject(error.message) else invoke.resolveObject(value ?: JSObject())
                } else invoke.resolveObject(result)
            }
            catch (error: Exception) { invoke.reject(error.message ?: error.javaClass.simpleName) }
        }
    }
}
