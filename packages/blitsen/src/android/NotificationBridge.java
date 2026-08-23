package com.blitsen.runtime;

import android.app.NativeActivity;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.util.Log;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.regex.Pattern;

/**
 * The smallest Java seam Android notification lifecycle requires.
 *
 * NativeActivity remains the owner of the Activity and every native lifecycle
 * callback. This subclass only retains a new Intent, which platform
 * NativeActivity does not do, and the receiver only persists a delete Intent.
 * Rust drains the resulting envelopes on a frame turn.
 */
public final class NotificationBridge {
    private static final String TAG = "BlitsenNotify";
    private static final String ACTIVATION_EXTRA = "blitsen.notification.activation";
    private static final String NONCE_EXTRA = "blitsen.notification.nonce";
    private static final String INBOX = "notification-activation-inbox";
    private static final Pattern SAFE_NONCE = Pattern.compile("[0-9a-f-]{1,96}");

    private NotificationBridge() {}

    private static void persist(Context context, Intent intent) {
        if (intent == null) return;
        String envelope = intent.getStringExtra(ACTIVATION_EXTRA);
        String nonce = intent.getStringExtra(NONCE_EXTRA);
        if (envelope == null || nonce == null || !SAFE_NONCE.matcher(nonce).matches()) return;

        File directory = new File(context.getFilesDir(), INBOX);
        if (!directory.isDirectory() && !directory.mkdirs()) {
            Log.e(TAG, "could not create notification activation inbox " + directory);
            return;
        }
        File destination = new File(directory, nonce + ".json");
        if (destination.isFile()) return;

        File temporary = null;
        try {
            temporary = File.createTempFile("activation-" + nonce + "-", ".tmp", directory);
            try (FileOutputStream output = new FileOutputStream(temporary)) {
                output.write(envelope.getBytes(StandardCharsets.UTF_8));
                output.flush();
                output.getFD().sync();
            }
            if (!temporary.renameTo(destination) && !destination.isFile()) {
                throw new IOException("could not rename " + temporary + " to " + destination);
            }
        } catch (IOException error) {
            Log.e(TAG, "could not persist notification activation", error);
        } finally {
            if (temporary != null && temporary.exists() && !temporary.delete()) {
                Log.w(TAG, "could not remove notification activation temporary file " + temporary);
            }
        }
    }

    /** NativeActivity plus the one callback its implementation deliberately omits. */
    public static final class Activity extends NativeActivity {
        @Override
        protected void onNewIntent(Intent intent) {
            super.onNewIntent(intent);
            persist(this, intent);
            setIntent(intent);
        }
    }

    /** Receives a notification delete Intent without bringing the Activity forward. */
    public static final class DismissReceiver extends BroadcastReceiver {
        @Override
        public void onReceive(Context context, Intent intent) {
            persist(context, intent);
        }
    }
}
