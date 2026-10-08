/**
 * A bridge class that declares nativeStart as an older application did: loading the library
 * must fail, and this program exits with 3 when it does.
 */
public final class RiviumBridge {
    private RiviumBridge() {}

    public static native int nativeStart(String root, String config);

    public static native int nativeStop(long timeoutMillis);

    public static native int nativeStatus();

    public static native String nativeLastError();

    public static native String nativeVersion();

    public static void main(String[] args) {
        try {
            System.loadLibrary(args[0]);
        } catch (UnsatisfiedLinkError expected) {
            System.out.println("jvm contract: the mismatch fails to load: " + expected);
            // The message names the method declared otherwise.
            System.exit(expected.getMessage().contains("nativeStart") ? 3 : 2);
        }
        System.out.println("jvm contract: the mismatch loaded");
        System.exit(1);
    }
}
