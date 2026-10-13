//! JVM contract preparation, kept out of the published library and its build script.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use jni::{jni_sig, jni_str, InitArgsBuilder, JNIVersion, JavaVM};
use rivium_test::process::Running;

use crate::test_root;

static JVM: OnceLock<JavaVM> = OnceLock::new();
const CHILD_ROOT: &str = "RIVIUM_TEST_JVM_ROOT";

/// Runs `jvm_bridge_contract` in an isolated copy of its own test executable.
///
/// The existing process runner bounds the whole JVM test and reaps it before deleting its files.
/// No cdylib is required: the child creates its JVM and registers this executable's native methods.
pub fn contract(sources: &Path, on_load: fn(&JavaVM) -> jni::sys::jint) {
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        in_process(&PathBuf::from(root), on_load);
        return;
    }
    let root = test_root();
    prepare(sources, root.path());
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([
            "--exact",
            "jvm_bridge_contract",
            "--nocapture",
            "--color",
            "never",
        ])
        .env(CHILD_ROOT, root.path());
    let exited = Running::start(command).wait();
    assert_eq!(
        exited.code,
        Some(0),
        "JVM contract failed:\n{}\n{}",
        exited.stdout,
        exited.stderr
    );
    assert!(
        exited.stdout.contains("jvm invocation contract: passed"),
        "Java contract did not complete:\n{}",
        exited.stdout
    );
    for output in [&exited.stdout, &exited.stderr] {
        assert!(
            !output.contains("WARNING in native method") && !output.contains("WARNING: JNI"),
            "JNI checker warning:\n{output}"
        );
    }
    assert!(
        exited.stdout.contains("1 passed; 0 failed"),
        "the child did not run the contract:\n{}",
        exited.stdout
    );
}

fn java_home() -> PathBuf {
    let home = PathBuf::from(
        std::env::var_os("JAVA_HOME")
            .expect("JVM tests require JAVA_HOME pointing to a full JDK (CI uses JDK 21)"),
    );
    assert!(home.is_absolute(), "JAVA_HOME must be an absolute JDK path");
    home
}

fn prepare(sources: &Path, root: &Path) {
    let home = java_home();
    let javac = home
        .join("bin")
        .join(format!("javac{}", std::env::consts::EXE_SUFFIX));
    assert!(
        javac.is_file(),
        "JDK compiler not found: {}",
        javac.display()
    );
    let classes = root.join("classes");
    let java = root.join("src/rivium/tests");
    fs::create_dir_all(&classes).unwrap();
    fs::create_dir_all(&java).unwrap();
    let mut compile = Command::new(javac);
    compile
        .args(["--release", "17", "-encoding", "UTF-8", "-proc:none", "-d"])
        .arg(&classes)
        .arg("--class-path")
        .arg(&classes);
    for name in ["RiviumBridge.java", "Contract.java", "ProbeContract.java"] {
        let source = fs::read_to_string(sources.join(name)).expect("read Java contract source");
        let file = java.join(name);
        fs::write(&file, format!("package rivium.tests;\n\n{source}")).unwrap();
        compile.arg(file);
    }
    let compiled = Running::start(compile).wait();
    assert_eq!(compiled.code, Some(0), "javac failed:\n{}", compiled.stderr);

    for name in ["RiviumBridge", "Contract", "ProbeContract"] {
        assert!(
            classes.join(format!("rivium/tests/{name}.class")).is_file(),
            "javac produced no {name}.class"
        );
    }
}

fn in_process(root: &Path, on_load: fn(&JavaVM) -> jni::sys::jint) {
    let home = java_home();
    let library = if cfg!(windows) {
        home.join("bin/server/jvm.dll")
    } else if cfg!(target_os = "macos") {
        home.join("lib/server/libjvm.dylib")
    } else {
        home.join("lib/server/libjvm.so")
    };
    assert!(
        library.is_file(),
        "JVM library not found: {}",
        library.display()
    );

    let classes = root.join("classes");
    let vm = JVM.get_or_init(|| {
        let args = InitArgsBuilder::new()
            .version(JNIVersion::V1_8)
            .option("-Xcheck:jni")
            .option("--enable-native-access=ALL-UNNAMED")
            .option("-XX:-CreateCoredumpOnCrash")
            .option(format!("-XX:ErrorFile={}/hs_err_pid%p.log", root.display()))
            .option(format!("-Djava.class.path={}", classes.display()))
            .build()
            .expect("JVM arguments");
        JavaVM::with_libjvm(args, || Ok(library)).expect("create the JVM from JAVA_HOME")
    });
    let app_root = root.join("service-服务-😀");
    fs::create_dir_all(&app_root).unwrap();
    let result = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        let version = on_load(vm);
        if version != jni::sys::JNI_VERSION_1_6 {
            assert_eq!(version, jni::sys::JNI_ERR, "unexpected load failure code");
            assert!(
                env.exception_check(),
                "load failure lost its Java exception"
            );
            eprintln!("native registration failed: JNI_ERR with a pending Java exception");
            return Err(jni::errors::Error::JavaException);
        }
        assert!(
            !env.exception_check(),
            "successful load left a Java exception"
        );
        let path = env.new_string(app_root.to_str().expect("UTF-8 temporary root"))?;
        env.call_static_method(
            jni_str!("rivium/tests/ProbeContract"),
            jni_str!("run"),
            jni_sig!("(Ljava/lang/String;)V"),
            &[(&path).into()],
        )?
        .v()?;
        Ok(())
    });
    if let Err(error) = result {
        // The embedded host installs a panic hook that writes to its log, not test stderr.
        eprintln!("Java bridge contract failed: {error:#?}");
        panic!("Java bridge contract failed");
    }
}
