package rivium.tests;
public final class Bridge {
    public static native int nativeStart(String[] args);
    public static native int nativeStop(long timeout);
    public static native int nativeStatus();
    public static native String nativeLastError();
    public static native String nativeVersion();
    static void require(boolean ok, String message) { if (!ok) throw new AssertionError(message); }
    public static void main(String[] args) throws InterruptedException {
        System.load(args[0]);
        if (args.length > 2 && args[2].equals("fault")) {
            require(nativeStart(null) == -99, "start panic");
            require(nativeStop(0) == -99, "stop panic");
            require(nativeStatus() == -99, "status panic");
            require(nativeLastError().isEmpty(), "error panic");
            require(nativeVersion().isEmpty(), "version panic");
            System.out.println("JNI panic containment passed");
            return;
        }
        require(nativeStatus() == 0, "initial status");
        require(nativeVersion().startsWith("rivium-demo "), "version");
        require(nativeStop(100) == 2, "idle stop");
        require(nativeStart(null) == -64, "null args require explicit root");
        require(nativeStart(new String[]{"--root", args[1], null}) == -64, "null argument");
        require(nativeStart(new String[]{"--root", args[1], "--set", "lifecycle.stop_timeout=0s"}) == -78, "invalid stop config");
        require(nativeStart(new String[]{"--root", args[1], "--secret-DO_NOT_DISCLOSE-😀"}) == -64, "unicode invalid option");
        require(!nativeLastError().contains("DO_NOT_DISCLOSE"), "diagnostics are redacted at Java ABI");
        require(nativeStart(new String[]{"--wrong"}) == -64, "invalid arguments");
        require(nativeStart(new String[]{"--root", args[1], "--version"}) == -64, "embedded rejects process inspection");
        for (int i=0; i<3; i++) {
            require(nativeStart(new String[]{"--root", args[1], "--set", "mode=中文😀"}) == 0, "start");
            require(nativeStatus() == 2, "running status");
            require(nativeStart(new String[]{"--root", args[1]}) == 1, "duplicate start");
            require(nativeLastError().isEmpty(), "successful generation clears old diagnostics");
            require(nativeStop(2000) == 0, "stop");
            require(nativeStatus() == 0, "idle status");
        }
        require(nativeStart(new String[]{"--root", args[1]}) == 0, "start before immediate stop");
        require(nativeStop(-1) == -124, "negative timeout sends stop without waiting");
        long deadline = System.nanoTime() + 5_000_000_000L;
        int restarted;
        do {
            Thread.sleep(5);
            restarted = nativeStart(new String[]{"--root", args[1]});
        } while ((restarted == 1 || restarted == 3) && System.nanoTime() < deadline);
        require(restarted == 0, "immediate stop eventually releases resources for restart: " + restarted);
        require(nativeStop(2000) == 0, "stop after immediate-stop recovery");
        require(nativeStart(new String[]{"--root", args[1], "--set", "mode=startup-error"}) == -69, "startup failure");
        require(!nativeLastError().isEmpty(), "last error");
        require(nativeStatus() == 5, "failed status");
        require(nativeStop(-1) == 2, "negative timeout while idle");
        System.out.println("JNI lifecycle and Unicode passed");
    }
}
