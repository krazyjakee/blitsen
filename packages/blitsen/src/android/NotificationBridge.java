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
 * NativeActivity remains the platform-owned launcher. The private receiver is
 * the only component allowed to read activation extras: it persists them, then
 * starts NativeActivity with a new Intent containing no trusted data. Rust
 * drains the resulting envelopes on a frame turn.
 */
public final class NotificationBridge {
    private static final String TAG = "BlitsenNotify";
    private static final String ACTIVATION_EXTRA = "blitsen.notification.activation";
    private static final String NONCE_EXTRA = "blitsen.notification.nonce";
    private static final String LAUNCH_EXTRA = "blitsen.notification.launch";
    private static final String INBOX = "notification-activation-inbox";
    private static final Pattern SAFE_NONCE = Pattern.compile("[0-9a-f-]{1,96}");

    private NotificationBridge() {}

    private static boolean persist(Context context, Intent intent) {
        if (intent == null) return false;
        String envelope = intent.getStringExtra(ACTIVATION_EXTRA);
        String nonce = intent.getStringExtra(NONCE_EXTRA);
        if (envelope == null || nonce == null || !SAFE_NONCE.matcher(nonce).matches()) return false;

        File directory = new File(context.getFilesDir(), INBOX);
        if (!directory.isDirectory() && !directory.mkdirs()) {
            Log.e(TAG, "could not create notification activation inbox " + directory);
            return false;
        }
        File destination = new File(directory, nonce + ".json");
        if (destination.isFile()) return true;

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
            return true;
        } catch (IOException error) {
            Log.e(TAG, "could not persist notification activation", error);
            return false;
        } finally {
            if (temporary != null && temporary.exists() && !temporary.delete()) {
                Log.w(TAG, "could not remove notification activation temporary file " + temporary);
            }
        }
    }

    /** The only trusted notification entry point; it is private to PendingIntents from this app. */
    public static final class ActivationReceiver extends BroadcastReceiver {
        @Override
        public void onReceive(Context context, Intent intent) {
            if (!persist(context, intent) || !intent.getBooleanExtra(LAUNCH_EXTRA, false)) return;
            Intent launch = new Intent(context, NativeActivity.class);
            launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP);
            context.startActivity(launch);
        }
    }
}
