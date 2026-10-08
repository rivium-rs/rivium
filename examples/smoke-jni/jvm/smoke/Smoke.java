package smoke;

/** Java side of the JNI smoke test: loads the cdylib and calls the registered native methods. */
public final class Smoke {
    static native String nativeVersion();

    static native int nativePanic();

    public static void main(String[] args) {
        System.loadLibrary("smoke_jni");
        String version = nativeVersion();
        if (!version.startsWith("smoke-jni ")) {
            throw new AssertionError("unexpected version: " + version);
        }
        int code = nativePanic();
        if (code != -99) {
            throw new AssertionError("panic was not converted to -99: " + code);
        }
        System.out.println("jvm smoke ok: " + version + ", panic -> " + code + ", JVM alive");
    }
}
