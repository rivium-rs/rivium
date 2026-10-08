//! The beacon service library inside an Android app: its JNI library is this one line. The app
//! declares the native methods on `com.example.beacon.RiviumBridge` (see rivium-jni), loads
//! `libbeacon_jni.so`, and starts the service on a background thread with `--root` set to its
//! files directory and `--set beacon.serial=…` set to the device's serial number.
#![forbid(unsafe_code)]

rivium_jni::export!(
    class = "com/example/beacon/RiviumBridge",
    app = beacon::Beacon
);
