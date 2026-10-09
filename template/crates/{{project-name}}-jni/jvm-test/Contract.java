import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.stream.Stream;

/**
 * The desktop JVM contract of a JNI library built with {@code rivium_jni::export!}: its bridge
 * class's native methods, called as an Android app calls them. The application must run below
 * an empty directory with its defaults and the start arguments that follow the mode, such as
 * {@code --set} overrides that its defaults need in a test. run.sh adds the package of the
 * bridge class.
 *
 * <p>Usage: {@code Contract <library name> <empty root directory> [contract [start argument]... |
 * panics-a | panics-b]}. The panic modes need a library built with {@code --cfg
 * rivium_jni_fault} and {@code RIVIUM_JNI_PANIC} naming the methods that panic.
 */
public final class Contract {
    private Contract() {}

    public static void main(String[] args) throws Exception {
        System.loadLibrary(args[0]);
        Path root = Path.of(args[1]);
        String mode = args.length > 2 ? args[2] : "contract";
        String[] extra = args.length > 3 ? Arrays.copyOfRange(args, 3, args.length) : new String[0];
        switch (mode) {
            case "contract" -> contract(root, extra);
            case "panics-a" -> panicsInStatusAndLastError(root);
            case "panics-b" -> panicsInStartStopAndVersion(root);
            default -> throw new IllegalArgumentException("unknown mode " + mode);
        }
        System.out.println("jvm contract (" + mode + "): passed");
    }

    private static void contract(Path root, String[] extra) throws Exception {
        String version = RiviumBridge.nativeVersion();
        check(version.matches("\\S+ \\S+"), "nativeVersion: " + version);
        Path log = log(root, version);
        expect(RiviumBridge.nativeStatus(), 0, "nativeStatus before a start");
        check(RiviumBridge.nativeLastError().isEmpty(), "no last error before a start");
        expect(RiviumBridge.nativeStop(4000), 2, "nativeStop before a start");

        // A null array counts as no arguments, a null argument as an empty one.
        expect(RiviumBridge.nativeStart(null), -64, "nativeStart(null)");
        check(lastError().equals("Usage: the embedded host must pass --root"), lastError());
        String[] nulls = start(root, extra, (String) null);
        expect(RiviumBridge.nativeStart(nulls), -64, "nativeStart with a null argument");
        String[] invalid = start(root, extra, "--set", "lifecycle.stop_timeout=0s");
        expect(RiviumBridge.nativeStart(invalid), -78, "nativeStart with an invalid configuration");
        check(lastError().startsWith("Config: invalid configuration: lifecycle.stop_timeout"),
                lastError());

        String[] rooted = start(root, extra);
        for (int round = 1; round <= 3; round++) {
            expect(RiviumBridge.nativeStart(rooted), 0, "nativeStart");
            expect(RiviumBridge.nativeStatus(), 2, "nativeStatus while running");
            check(lastError().isEmpty(), "a start that runs clears the last error");
            expect(RiviumBridge.nativeStart(rooted), 1, "a second nativeStart");
            long asked = System.nanoTime();
            expect(RiviumBridge.nativeStop(4000), 0, "nativeStop");
            long took = (System.nanoTime() - asked) / 1_000_000;
            check(took < 4000, "nativeStop took " + took + " ms");
            expect(RiviumBridge.nativeStatus(), 0, "nativeStatus after a stop");
            expect(RiviumBridge.nativeStop(4000), 2, "a second nativeStop");
            long stopped = Files.readAllLines(log).stream()
                    .filter(line -> line.contains("stopped code=\"Ok\""))
                    .count();
            check(stopped == round, "round " + round + ": the last lines are in " + log);
        }
    }

    /** RIVIUM_JNI_PANIC=nativeStatus,nativeLastError: -99 and an empty string, logged. */
    private static void panicsInStatusAndLastError(Path root) throws Exception {
        expect(RiviumBridge.nativeStatus(), -99, "nativeStatus");
        check(RiviumBridge.nativeLastError().isEmpty(), "nativeLastError");
        Path log = log(root, RiviumBridge.nativeVersion());
        expect(RiviumBridge.nativeStart(new String[] {"--root", root.toString()}), 0, "nativeStart");
        // Logging is installed now, and so is Rivium's panic hook.
        expect(RiviumBridge.nativeStatus(), -99, "nativeStatus while running");
        expect(RiviumBridge.nativeStop(4000), 0, "nativeStop");
        check(Files.readString(log).contains("nativeStatus: a panic injected by a test build"),
                "the panic is in " + log);
    }

    /** RIVIUM_JNI_PANIC=nativeStart,nativeStop,nativeVersion: -99 and an empty string. */
    private static void panicsInStartStopAndVersion(Path root) {
        expect(RiviumBridge.nativeStart(new String[] {"--root", root.toString()}), -99, "nativeStart");
        expect(RiviumBridge.nativeStop(4000), -99, "nativeStop");
        check(RiviumBridge.nativeVersion().isEmpty(), "nativeVersion");
        expect(RiviumBridge.nativeStatus(), 0, "nativeStatus: the JVM goes on, the host idle");
    }

    /** The arguments of a start: the root, then {@code extra}, then {@code more}, which win. */
    private static String[] start(Path root, String[] extra, String... more) {
        Stream<String> rooted = Stream.of("--root", root.toString());
        return Stream.concat(Stream.concat(rooted, Arrays.stream(extra)), Arrays.stream(more))
                .toArray(String[]::new);
    }

    /** The main log file of the service, named in {@code version}. */
    private static Path log(Path root, String version) {
        String name = version.split(" ")[0];
        return root.resolve("logs").resolve(name).resolve(name + ".log");
    }

    private static String lastError() {
        return RiviumBridge.nativeLastError();
    }

    private static void expect(int code, int expected, String call) {
        if (code != expected) {
            throw new AssertionError(
                    call + " returned " + code + ", not " + expected + "; last error: " + lastError());
        }
    }

    private static void check(boolean condition, String what) {
        if (!condition) {
            throw new AssertionError(what);
        }
    }
}
