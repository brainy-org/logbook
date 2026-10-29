// The app's Rust side calls these while it records (apps/desktop/src-tauri/
// src/android.rs): the permissions first, then the service started with the
// recording and stopped once its meetings have finished.

package io.github.brainy_org.logbook.recording

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.PowerManager
import android.provider.Settings
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class StartArgs {
  var title: String? = null
  /** Told when the notification's Stop is pressed. */
  var onStop: Channel? = null
}

@InvokeArg
class UpdateArgs {
  var text: String = "Recording"
  /** With the notification's Stop: while recording. */
  var stoppable: Boolean = false
}

@InvokeArg
class AskArgs {
  /** Ask only if it was never asked on this phone. */
  var once: Boolean = false
}

@TauriPlugin(
  permissions = [
    Permission(strings = [Manifest.permission.RECORD_AUDIO], alias = "microphone"),
    // Android 13 and later; refused at once on older ones, which show the
    // notification anyway.
    Permission(strings = [Manifest.permission.POST_NOTIFICATIONS], alias = "notifications"),
  ]
)
class RecordingPlugin(private val activity: Activity) : Plugin(activity) {
  /** The foreground service and its wake lock, from now until `stop`. */
  @Command
  fun start(invoke: Invoke) {
    val args = invoke.parseArgs(StartArgs::class.java)
    val channel = args.onStop
    RecordingService.onStop = channel?.let { c -> { c.send(JSObject()) } }
    val intent = Intent(activity, RecordingService::class.java)
      .setAction(RecordingService.ACTION_START)
      .putExtra(RecordingService.EXTRA_TITLE, args.title)
    try {
      // While the app is in the foreground: Android 12 and later refuse
      // to start a foreground service from the background, and 14 a
      // microphone one without RECORD_AUDIO granted.
      ContextCompat.startForegroundService(activity, intent)
      invoke.resolve()
    } catch (e: Exception) {
      RecordingService.onStop = null
      invoke.reject("the recording service could not start: ${e.message}")
    }
  }

  /** The notification's line: recording, finishing. */
  @Command
  fun update(invoke: Invoke) {
    val args = invoke.parseArgs(UpdateArgs::class.java)
    RecordingService.instance?.update(args.text, args.stoppable)
    invoke.resolve()
  }

  @Command
  fun stop(invoke: Invoke) {
    RecordingService.onStop = null
    activity.stopService(Intent(activity, RecordingService::class.java))
    invoke.resolve()
  }

  /** Whether the app is exempt from battery optimization. */
  @Command
  fun background(invoke: Invoke) {
    invoke.resolve(JSObject().put("unrestricted", unrestricted()))
  }

  /**
   * Asks the person to exempt the app from battery optimization (the
   * system's dialog); with `once`, only the first time. Answers with the
   * state before the dialog: it does not wait for it.
   */
  @Command
  fun askBackground(invoke: Invoke) {
    val args = invoke.parseArgs(AskArgs::class.java)
    val prefs = activity.getSharedPreferences("recording", Context.MODE_PRIVATE)
    val unrestricted = unrestricted()
    if (!unrestricted && !(args.once && prefs.getBoolean("asked-background", false))) {
      prefs.edit().putBoolean("asked-background", true).apply()
      try {
        val intent = Intent(
          Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
          Uri.parse("package:${activity.packageName}"),
        )
        activity.startActivity(intent)
      } catch (e: Exception) {
        // Some builds have no such dialog: the list of apps instead.
        try {
          activity.startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
        } catch (_: Exception) {
        }
      }
    }
    invoke.resolve(JSObject().put("unrestricted", unrestricted))
  }

  private fun unrestricted(): Boolean {
    val power = activity.getSystemService(Context.POWER_SERVICE) as PowerManager
    return power.isIgnoringBatteryOptimizations(activity.packageName)
  }
}
