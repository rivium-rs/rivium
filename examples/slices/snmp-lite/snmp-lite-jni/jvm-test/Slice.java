import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.util.Arrays;

/**
 * snmp-lite in a desktop JVM, driven by the business tests (snmp-lite-bin/tests/hosts.rs): it
 * starts the service with the arguments given, as the app does, prints {@code running}, and
 * waits for a line on standard input. Then it stops the service and prints {@code stopped};
 * after the line {@code again} it starts it once more. The test adds the package of the bridge.
 *
 * <p>Usage: {@code Slice <library name> [start argument]...}
 */
public final class Slice {
    private Slice() {}

    public static void main(String[] args) throws Exception {
        System.loadLibrary(args[0]);
        String[] start = Arrays.copyOfRange(args, 1, args.length);
        BufferedReader in = new BufferedReader(new InputStreamReader(System.in));
        String next;
        do {
            expect(RiviumBridge.nativeStart(start), 0, "nativeStart");
            expect(RiviumBridge.nativeStatus(), 2, "nativeStatus while running");
            System.out.println("running");
            next = in.readLine();
            expect(RiviumBridge.nativeStop(4000), 0, "nativeStop");
            expect(RiviumBridge.nativeStatus(), 0, "nativeStatus after the stop");
            System.out.println("stopped");
        } while ("again".equals(next));
    }

    private static void expect(int code, int expected, String call) {
        if (code != expected) {
            throw new AssertionError(call + " returned " + code + ", not " + expected
                    + "; last error: " + RiviumBridge.nativeLastError());
        }
    }
}
