package io.github.weiking1021.scrcpyagent.android;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.media.AudioFocusRequest;
import android.media.AudioManager;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;

/** Keeps shared mirror audio eligible for playback and audio focus while using overlay windows. */
public final class AudioPlaybackService extends Service {
    private AudioManager manager;
    private AudioFocusRequest focus;

    @Override public void onCreate() {
        super.onCreate();
        try { startPlayback(); }
        catch (RuntimeException error) {
            DeviceAudio.focus(0, "service error: " + error.getClass().getSimpleName());
            stopSelf();
        }
    }

    private void startPlayback() {
        var notifications = getSystemService(NotificationManager.class);
        notifications.createNotificationChannel(new NotificationChannel("mirror-audio", getString(R.string.audio_channel), NotificationManager.IMPORTANCE_LOW));
        var launch = getPackageManager().getLaunchIntentForPackage(getPackageName());
        var notification = new Notification.Builder(this, "mirror-audio")
            .setSmallIcon(android.R.drawable.ic_media_play).setContentTitle("ScrcpyAgent")
            .setContentText(getString(R.string.audio_notification)).setOngoing(true)
            .setContentIntent(PendingIntent.getActivity(this, 0, launch, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT)).build();
        startForeground(4102, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK);
        manager = getSystemService(AudioManager.class);
        focus = new AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN).setAudioAttributes(DeviceAudio.attributes())
            .setAcceptsDelayedFocusGain(true).setOnAudioFocusChangeListener(change -> {
                if (change == AudioManager.AUDIOFOCUS_GAIN) DeviceAudio.focus(1, "focus granted");
                else if (change == AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK) DeviceAudio.focus(0.2f, "ducked");
                else DeviceAudio.focus(0, "focus suspended");
            }, new Handler(Looper.getMainLooper())).build();
        int result = manager.requestAudioFocus(focus);
        DeviceAudio.focus(result == AudioManager.AUDIOFOCUS_REQUEST_GRANTED ? 1 : 0,
            result == AudioManager.AUDIOFOCUS_REQUEST_GRANTED ? "focus granted"
                : result == AudioManager.AUDIOFOCUS_REQUEST_DELAYED ? "waiting for focus" : "focus denied");
    }

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        if (!DeviceAudio.active()) stopSelf();
        return START_NOT_STICKY;
    }
    @Override public IBinder onBind(Intent intent) { return null; }
    @Override public void onDestroy() {
        if (manager != null && focus != null) manager.abandonAudioFocusRequest(focus);
        DeviceAudio.focus(0, "service stopped");
        super.onDestroy();
    }
}
