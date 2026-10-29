// What keeps a recording going with the screen off, the phone asleep and
// the app in the background: a foreground service of type microphone (its
// ongoing notification, with Stop), a partial wake lock for the CPU and a
// Wi-Fi lock for the network. The audio itself is taken in Rust (cpal on
// AAudio) in this same process; the service only keeps the process alive
// and allowed to use the microphone.

package io.github.brainy_org.logbook.recording

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat

class RecordingService : Service() {
  companion object {
    const val ACTION_START = "io.github.brainy_org.logbook.recording.START"
    const val ACTION_STOP = "io.github.brainy_org.logbook.recording.STOP"
    const val EXTRA_TITLE = "title"
    private const val CHANNEL = "recording"
    private const val NOTIFICATION = 1
    private const val TAG = "logbook:recording"

    /** The recording's stop, set by the plugin while it runs. */
    @Volatile var onStop: (() -> Unit)? = null
    /** The service while it runs, for the notification's updates. */
    @Volatile var instance: RecordingService? = null
  }

  private var wakeLock: PowerManager.WakeLock? = null
  private var wifiLock: WifiManager.WifiLock? = null
  private var title: String = "Logbook"
  private var started = false

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    if (intent?.action == ACTION_STOP) {
      // The recording stops; the service stays until its meetings have
      // finished (the plugin's `stop`).
      onStop?.invoke()
      if (started) update("Finishing…", false) else stopSelf()
      return START_NOT_STICKY
    }
    title = intent?.getStringExtra(EXTRA_TITLE)?.takeIf { it.isNotBlank() } ?: "Logbook"
    channel()
    val type =
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
      else 0
    ServiceCompat.startForeground(this, NOTIFICATION, notification("Recording", true), type)
    started = true
    instance = this
    hold()
    // Started again by the system after the process died, the recording
    // would not be there: never.
    return START_NOT_STICKY
  }

  /** The notification's line; with its Stop button when stoppable. */
  fun update(text: String, stoppable: Boolean) {
    if (!started) return
    val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    manager.notify(NOTIFICATION, notification(text, stoppable))
  }

  override fun onDestroy() {
    instance = null
    release()
    ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
    super.onDestroy()
  }

  private fun channel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    val channel = NotificationChannel(CHANNEL, "Recording", NotificationManager.IMPORTANCE_LOW)
    channel.description = "Shown while a meeting is recorded or finished"
    channel.setShowBadge(false)
    manager.createNotificationChannel(channel)
  }

  private fun notification(text: String, stoppable: Boolean): Notification {
    val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
    val open = packageManager.getLaunchIntentForPackage(packageName)?.let {
      PendingIntent.getActivity(this, 0, it, flags)
    }
    val stop = PendingIntent.getService(
      this,
      1,
      Intent(this, RecordingService::class.java).setAction(ACTION_STOP),
      flags,
    )
    val builder = NotificationCompat.Builder(this, CHANNEL)
      .setSmallIcon(android.R.drawable.ic_btn_speak_now)
      .setContentTitle(title)
      .setContentText(text)
      .setOngoing(true)
      .setOnlyAlertOnce(true)
      .setCategory(NotificationCompat.CATEGORY_SERVICE)
      .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
      .setContentIntent(open)
    if (stoppable) {
      builder.addAction(android.R.drawable.ic_media_pause, "Stop", stop)
    }
    return builder.build()
  }

  /** The CPU and Wi-Fi kept awake with the screen off. */
  private fun hold() {
    if (wakeLock == null) {
      val power = getSystemService(Context.POWER_SERVICE) as PowerManager
      wakeLock = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, TAG).apply {
        setReferenceCounted(false)
        // Released in onDestroy: a recording may last for days.
        acquire()
      }
    }
    if (wifiLock == null) {
      val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
      @Suppress("DEPRECATION")
      wifiLock = wifi.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, TAG).apply {
        setReferenceCounted(false)
        acquire()
      }
    }
  }

  private fun release() {
    wakeLock?.takeIf { it.isHeld }?.release()
    wakeLock = null
    wifiLock?.takeIf { it.isHeld }?.release()
    wifiLock = null
  }
}
