/** Additional checks using the private probe application's controllable service. */
public final class ProbeContract {
    private ProbeContract() {}

    public static void run(String root) throws Exception {
        if (!RiviumBridge.nativeVersion().equals("jvm-host 0.0.0")) {
            throw new AssertionError("the registered application has the wrong identity");
        }
        Contract.run(root);
        String[] args = {"--root", root, "--set", "ignore_stop=true"};
        if (RiviumBridge.nativeStart(args) != 0) {
            throw new AssertionError("cannot start the stop-timeout probe: " + RiviumBridge.nativeLastError());
        }
        long start = System.nanoTime();
        int stopped = RiviumBridge.nativeStop(-10_000);
        long elapsed = (System.nanoTime() - start) / 1_000_000;
        if (stopped != -124 || elapsed >= 1000) {
            throw new AssertionError("negative timeout was not clamped to zero: " + stopped + " in " + elapsed + " ms");
        }
        long deadline = System.nanoTime() + 5_000_000_000L;
        while (RiviumBridge.nativeStatus() != 0) {
            if (System.nanoTime() > deadline) {
                throw new AssertionError("the timed-out host did not finish teardown");
            }
            Thread.sleep(10);
        }
        if (!RiviumBridge.nativeLastError().startsWith("StopTimedOut:")) {
            throw new AssertionError("the timeout reason is missing: " + RiviumBridge.nativeLastError());
        }
        System.out.println("jvm invocation contract: passed");
    }
}
