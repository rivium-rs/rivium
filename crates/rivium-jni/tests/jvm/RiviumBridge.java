/**
 * The bridge class of a JNI library built with {@code rivium_jni::export!}: the reference for
 * the class an application declares. A class that declares these methods otherwise fails to
 * load. In Kotlin: {@code object RiviumBridge} with {@code @JvmStatic external fun} methods.
 * The package is the application's; run.sh adds it.
 */
public final class RiviumBridge {
    private RiviumBridge() {}

    /**
     * Starts the services and waits until they run or fail: never call it on the main thread.
     * Arguments: {@code --root DIR} (required), {@code --config FILE}, {@code --set KEY=VALUE}.
     * Returns 0 when running; 1 already running; 3 still stopping; 4 cancelled by a stop;
     * -64 bad arguments; -69 the services did not start; -71 no thread or runtime; -73 the
     * log directory cannot be written; -78 an invalid configuration; -99 a panic.
     */
    public static native int nativeStart(String[] args);

    /**
     * Stops the services within the timeout (at most 2000 ms on the main thread). Returns 0
     * when stopped, 2 not running, -124 a service abandoned or the stop late, -99 a panic.
     */
    public static native int nativeStop(long timeoutMillis);

    /** Idle 0, starting 1, running 2, stopping 3, restarting 4, failed 5; -99 a panic. */
    public static native int nativeStatus();

    /** {@code "<code name>: <message>"} of the last failure, or an empty string. */
    public static native String nativeLastError();

    /** {@code "<name> <version>"}. */
    public static native String nativeVersion();
}
