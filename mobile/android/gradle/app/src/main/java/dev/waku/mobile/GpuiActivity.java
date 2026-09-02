package dev.waku.mobile;

import android.app.NativeActivity;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.content.pm.PackageManager;
import android.media.AudioManager;
import android.net.Uri;
import android.os.Bundle;
import android.text.Editable;
import android.text.TextWatcher;
import android.util.Log;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.inputmethod.EditorInfo;
import android.widget.EditText;

import androidx.core.splashscreen.SplashScreen;

/**
 * Custom Activity extending NativeActivity that integrates with the
 * AndroidX SplashScreen API.
 *
 * On API 31+ the system splash screen is displayed automatically via theme
 * attributes. On API 26-30 the AndroidX compat library emulates the same
 * behavior using the theme's windowBackground drawable.
 *
 * The splash screen is held visible until the Rust native library signals
 * that initialization is complete by setting NATIVE_INITIALIZED to true
 * (see src/android/jni.rs). This prevents the user from seeing an empty
 * or partially-rendered surface during startup.
 *
 * Also handles:
 * - Deep link intents (onNewIntent)
 * - Volume key routing to the MUSIC audio stream
 * - Media button events via MediaSessionCompat
 */
public class GpuiActivity extends NativeActivity {

    /** Whether the native .so has been loaded via System.loadLibrary. */
    private static volatile boolean sNativeLibLoaded = false;

    /**
     * IME target: a hidden EditText that receives the soft keyboard's
     * `commitText` calls. NativeActivity has no InputConnection, so paste
     * (which many keyboards deliver as one commitText, not key events)
     * would otherwise be lost. The committed text is forwarded to the
     * native side via {@link #nativeCommitText(String)}.
     */
    private EditText mImeTarget;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // Install the splash screen BEFORE calling super.onCreate().
        // This is required by the AndroidX SplashScreen API.
        SplashScreen splash = SplashScreen.installSplashScreen(this);

        // NativeActivity loads the .so via dlopen (loadNativeCode), which does
        // NOT register JNI symbols with the classloader. We must call
        // System.loadLibrary() ourselves so that JNI can resolve our native
        // methods. Reading the library name from the manifest meta-data ensures
        // we stay in sync with the nativeLibraryName placeholder.
        if (!sNativeLibLoaded) {
            try {
                ActivityInfo ai = getPackageManager().getActivityInfo(
                        getComponentName(), PackageManager.GET_META_DATA);
                String libName = ai.metaData.getString("android.app.lib_name");
                if (libName != null) {
                    System.loadLibrary(libName);
                    sNativeLibLoaded = true;
                }
            } catch (PackageManager.NameNotFoundException e) {
                // Shouldn't happen — we're querying our own activity.
            } catch (UnsatisfiedLinkError e) {
                // Library may already be loaded by NativeActivity; that's fine.
                sNativeLibLoaded = true;
            }
        }

        // Keep the splash screen visible until the native side signals readiness.
        splash.setKeepOnScreenCondition(() -> !isNativeReady());

        // Route volume keys to the MUSIC stream so they control media volume
        // rather than the ringer/notification volume.
        setVolumeControlStream(AudioManager.STREAM_MUSIC);

        sInstance = this;
        super.onCreate(savedInstanceState);

        // Hidden IME target so the soft keyboard's paste / autocorrect
        // (delivered as commitText) reaches the native text input callback.
        // It is added to the window (off-screen) so it can hold IME focus;
        // NativeActivity itself cannot serve an InputConnection.
        mImeTarget = new EditText(this) {
            @Override
            public android.view.inputmethod.InputConnection onCreateInputConnection(
                    EditorInfo outAttrs) {
                android.view.inputmethod.InputConnection base = super.onCreateInputConnection(outAttrs);
                if (base == null) {
                    return null;
                }
                // Intercept commitText: a soft-keyboard paste (and
                // autocorrect) arrives here instead of via key events, and
                // must be forwarded to native as text input.
                return new android.view.inputmethod.InputConnectionWrapper(base, true) {
                    @Override
                    public boolean commitText(CharSequence text, int newCursorPosition) {
                        if (text != null && text.length() > 0) {
                            Log.i("GpuiActivity", "IME commitText: " + text.length() + " chars");
                            try {
                                nativeCommitText(text.toString());
                            } catch (Throwable t) {
                                Log.e("GpuiActivity", "nativeCommitText failed", t);
                            }
                        }
                        return true; // consumed; don't also insert into EditText
                    }
                };
            }
        };
        // The IME target must stay VISIBLE for Android to route
        // commitText (paste/autocorrect) to it — an INVISIBLE view is
        // ignored by InputMethodManager. It's placed off-screen
        // (-10,-10) and backgroundless so the user never sees it.
        mImeTarget.setVisibility(View.VISIBLE);
        mImeTarget.setBackgroundColor(android.graphics.Color.TRANSPARENT);
        mImeTarget.setFocusable(true);
        mImeTarget.setFocusableInTouchMode(true);
        mImeTarget.setSingleLine(true);
        mImeTarget.setImeOptions(EditorInfo.IME_ACTION_DONE);
        mImeTarget.setRawInputType(android.text.InputType.TYPE_CLASS_TEXT);
        mImeTarget.addTextChangedListener(new TextWatcher() {
            @Override
            public void beforeTextChanged(CharSequence s, int start, int count, int after) {}

            @Override
            public void onTextChanged(CharSequence s, int start, int before, int count) {}

            @Override
            public void afterTextChanged(Editable s) {
                // commitText is already forwarded by the
                // InputConnectionWrapper above; just keep the hidden
                // EditText empty so the next paste starts clean. This also
                // catches any input path that mutated the text directly.
                if (s.length() > 0) {
                    mImeTarget.setText("");
                }
            }
        });
        // Position it off-screen so it never paints over the GPUI surface.
        android.widget.FrameLayout.LayoutParams lp = new android.widget.FrameLayout.LayoutParams(
                1, 1, android.view.Gravity.TOP | android.view.Gravity.START);
        lp.leftMargin = -10;
        lp.topMargin = -10;
        addContentView(mImeTarget, lp);
    }

    /**
     * Check if the native library is fully initialized.
     * Returns false if the .so hasn't been loaded yet or if
     * NATIVE_INITIALIZED hasn't been set to true.
     */
    private boolean isNativeReady() {
        if (!sNativeLibLoaded) {
            return false;
        }
        try {
            return nativeIsInitialized();
        } catch (UnsatisfiedLinkError e) {
            return false;
        }
    }

    /**
     * Intercept key events to handle volume and media buttons.
     *
     * NativeActivity normally forwards ALL key events to the native side,
     * which means volume keys would be consumed by the Rust event loop
     * without actually adjusting the system volume. We intercept them here
     * and let the system handle them instead.
     */
    @Override
    public boolean dispatchTouchEvent(MotionEvent ev) {
        // Re-assert IME focus on every touch: NativeActivity's native
        // surface grabs window focus, which would otherwise detach the
        // hidden EditText from the IME and drop commitText (paste).
        if (ev.getAction() == MotionEvent.ACTION_DOWN) {
            if (mImeTarget != null && !mImeTarget.hasFocus()) {
                mImeTarget.requestFocus();
            }
        }
        return super.dispatchTouchEvent(ev);
    }

    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        int keyCode = event.getKeyCode();
        switch (keyCode) {
            case KeyEvent.KEYCODE_VOLUME_UP:
            case KeyEvent.KEYCODE_VOLUME_DOWN:
            case KeyEvent.KEYCODE_VOLUME_MUTE:
                // Let the system handle volume keys (adjusts STREAM_MUSIC).
                // Don't pass to NativeActivity's native input handler.
                return super.dispatchKeyEvent(event);

            case KeyEvent.KEYCODE_MEDIA_PLAY:
            case KeyEvent.KEYCODE_MEDIA_PAUSE:
            case KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE:
            case KeyEvent.KEYCODE_MEDIA_NEXT:
            case KeyEvent.KEYCODE_MEDIA_PREVIOUS:
            case KeyEvent.KEYCODE_MEDIA_STOP:
            case KeyEvent.KEYCODE_HEADSETHOOK:
                // Route media buttons through the MediaSession.
                // MediaButtonReceiver will dispatch to our session callback.
                return super.dispatchKeyEvent(event);

            default:
                return super.dispatchKeyEvent(event);
        }
    }

    @Override
    protected void onDestroy() {
        // Release media session when activity is destroyed.
        GpuiMediaSession.release();
        super.onDestroy();
    }

    /**
     * Handle new intents delivered to this singleTask activity.
     *
     * When the app is already running and a deeplink is opened
     * (e.g. `adb shell am start -d gpui://video_player`), this method
     * receives the new intent. We update the activity's intent and
     * notify the Rust side via JNI.
     */
    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);

        Uri data = intent.getData();
        if (data != null) {
            String url = data.toString();
            Log.i("GpuiActivity", "onNewIntent deeplink: " + url);
            try {
                nativeOnDeepLink(url);
            } catch (UnsatisfiedLinkError e) {
                Log.w("GpuiActivity", "nativeOnDeepLink not available yet");
            }
        }
    }

    /**
     * JNI bridge to check if the Rust NATIVE_INITIALIZED flag is set.
     */
    private static native boolean nativeIsInitialized();

    /**
     * JNI bridge to notify Rust of an incoming deeplink URL.
     */
    private static native void nativeOnDeepLink(String url);

    /**
     * JNI bridge to forward full-text IME commits (paste, autocorrect) to
     * the Rust text input callback.
     */
    private static native void nativeCommitText(String text);

    /**
     * Read the clipboard as plain text. Called from Rust when the user
     * explicitly pastes (paste button / long-press), which is reliable on
     * NativeActivity where IME commitText delivery is vendor-dependent.
     */
    public static String getClipboardText() {
        GpuiActivity activity = sInstance;
        if (activity == null) {
            return "";
        }
        android.content.ClipboardManager cm =
                (android.content.ClipboardManager)
                        activity.getSystemService(android.content.Context.CLIPBOARD_SERVICE);
        if (cm == null || !cm.hasPrimaryClip()) {
            return "";
        }
        android.content.ClipData clip = cm.getPrimaryClip();
        if (clip == null || clip.getItemCount() == 0) {
            return "";
        }
        CharSequence text = clip.getItemAt(0).coerceToText(activity);
        return text == null ? "" : text.toString();
    }

    /** Request code for the QR scanner activity result. */
    private static final int REQ_QR_SCAN = 0x5152;

    /**
     * Launch a QR scanner. Tries a few well-known scanner Intents (Google
     * ML Kit, ZXing) and falls back gracefully if none is installed. The
     * scanned text is forwarded to native via nativeCommitText so it fills
     * the ticket field.
     */
    public static void startQrScan() {
        GpuiActivity activity = sInstance;
        if (activity == null) {
            return;
        }
        activity.runOnUiThread(() -> {
            android.content.Intent intent = new android.content.Intent(
                    "com.google.zxing.client.android.SCAN");
            intent.putExtra("SCAN_MODE", "QR_CODE_MODE");
            try {
                activity.startActivityForResult(intent, REQ_QR_SCAN);
            } catch (android.content.ActivityNotFoundException e) {
                Log.w("GpuiActivity", "No QR scanner app installed");
                // TODO: surface a "no scanner installed" hint in the UI.
            }
        });
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, android.content.Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == REQ_QR_SCAN && resultCode == RESULT_OK && data != null) {
            String scanned = data.getStringExtra("SCAN_RESULT");
            if (scanned != null && !scanned.isEmpty()) {
                try {
                    nativeCommitText(scanned);
                } catch (Throwable t) {
                    Log.e("GpuiActivity", "nativeCommitText (scan) failed", t);
                }
            }
        }
    }

    /**
     * Called from Rust to focus the hidden IME target and (when
     * {@code showKeyboard}) pop the soft keyboard. Both run on the UI
     * thread after focus, so the IME binds the EditText — not the native
     * surface — before the keyboard is shown.
     */
    public static void requestImeFocus(boolean showKeyboard) {
        GpuiActivity activity = sInstance;
        if (activity == null || activity.mImeTarget == null) {
            return;
        }
        // View/IME operations must run on the UI thread; this JNI entry is
        // called from the native thread (touch + keyboard), so hop over.
        activity.runOnUiThread(() -> {
            activity.mImeTarget.requestFocus();
            android.view.inputmethod.InputMethodManager imm =
                    (android.view.inputmethod.InputMethodManager)
                            activity.getSystemService(android.content.Context.INPUT_METHOD_SERVICE);
            if (imm == null) {
                return;
            }
            // Rebind the IME to the EditText's InputConnection, then (only
            // when the field was tapped) show the keyboard anchored to it.
            imm.restartInput(activity.mImeTarget);
            if (showKeyboard) {
                imm.showSoftInput(activity.mImeTarget, 0);
            }
        });
    }

    private static volatile GpuiActivity sInstance;
}
