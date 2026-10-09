//! The snmp-lite service library inside an Android app: its JNI library is this one line. The
//! app declares the native methods on `com.example.snmplite.RiviumBridge` (see rivium-jni),
//! loads `libsnmp_lite_jni.so`, and starts the service on a background thread with `--root` set
//! to its files directory, `--set device.serial=…` to the device's serial number and
//! `--set snmp.addr=…` to a port that an app may bind.
#![forbid(unsafe_code)]

rivium_jni::export!(
    class = "com/example/snmplite/RiviumBridge",
    app = snmp_lite::SnmpLite
);
