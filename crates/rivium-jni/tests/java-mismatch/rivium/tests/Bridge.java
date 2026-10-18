package rivium.tests;
public final class Bridge {
    public static native int nativeStart(String wrongType);
    public static void main(String[] args) {
        try {
            System.load(args[0]);
            throw new AssertionError("mismatched native signatures loaded");
        } catch (LinkageError expected) {
            System.out.println("JNI mismatched signatures rejected");
        }
    }
}
