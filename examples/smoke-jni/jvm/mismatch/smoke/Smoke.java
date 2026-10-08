package smoke;

/**
 * Planted mismatch: declares nativePanic with a different signature than the library registers,
 * so loading the library must fail with UnsatisfiedLinkError.
 */
public final class Smoke {
    static native String nativeVersion();

    static native int nativePanic(String unexpected);

    public static void main(String[] args) {
        try {
            System.loadLibrary("smoke_jni");
        } catch (UnsatisfiedLinkError expected) {
            System.out.println("mismatch detected at load time: " + expected.getMessage());
            System.exit(3);
        }
        System.out.println("mismatch NOT detected");
    }
}
