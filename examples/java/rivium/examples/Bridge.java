package rivium.examples;

import java.nio.file.Path;

public final class Bridge {
    public static native int nativeStart(String[] args);
    public static native int nativeStop(long timeoutMillis);
    public static native int nativeStatus();
    public static native String nativeLastError();
    public static native String nativeVersion();

    public static void main(String[] args) {
        if (args.length != 2) {
            throw new IllegalArgumentException("Expected native library path and application root");
        }
        System.load(Path.of(args[0]).toAbsolutePath().toString());
        System.out.println(nativeVersion());
        int started = nativeStart(new String[] {"--root", args[1]});
        if (started != 0) {
            throw new IllegalStateException("Start failed: " + started + ": " + nativeLastError());
        }
        try {
            System.out.println("Running status: " + nativeStatus());
        } finally {
            int stopped = nativeStop(2000);
            if (stopped != 0) {
                throw new IllegalStateException("Stop failed: " + stopped + ": " + nativeLastError());
            }
        }
        System.out.println("Stopped status: " + nativeStatus());
    }
}
