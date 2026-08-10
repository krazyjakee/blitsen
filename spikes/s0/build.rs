use std::{env, path::PathBuf};

fn main() {
    println!("cargo::rerun-if-env-changed=CARGO_FEATURE_JSC");
    if env::var_os("CARGO_FEATURE_JSC").is_none() {
        return;
    }
    let lib_dir = PathBuf::from(
        env::var_os("BLITSEN_JSC_LIB_DIR")
            .expect("BLITSEN_JSC_LIB_DIR must point to Bun's static WebKit lib directory"),
    );
    let mimalloc_dir = PathBuf::from(
        env::var_os("BLITSEN_MIMALLOC_DIR")
            .expect("BLITSEN_MIMALLOC_DIR must point to Bun's mimalloc checkout"),
    );
    let mimalloc_source = mimalloc_dir.join("src/static.c");
    let mimalloc_include = mimalloc_dir.join("include");
    assert!(
        mimalloc_source.is_file(),
        "missing {}",
        mimalloc_source.display()
    );

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap();
    let mut mimalloc = cc::Build::new();
    mimalloc
        .cpp(true)
        .cargo_metadata(false)
        .file(&mimalloc_source)
        .include(&mimalloc_include)
        .define("MI_STATIC_LIB", None)
        .define("MI_SKIP_COLLECT_ON_EXIT", "1")
        .define("MI_NO_PROCESS_DETACH", "1")
        .define("MI_BUILD_RELEASE", None)
        .define("MI_DEFAULT_ALLOW_THP", "0");
    if target_env != "msvc" {
        mimalloc
            .flag("-fvisibility=hidden")
            .flag("-ftls-model=initial-exec");
    }
    mimalloc.compile("mimalloc");
    let mimalloc_archive =
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join(if target_env == "msvc" {
            "mimalloc.lib"
        } else {
            "libmimalloc.a"
        });

    println!("cargo::rerun-if-env-changed=BLITSEN_JSC_LIB_DIR");
    println!("cargo::rerun-if-env-changed=BLITSEN_MIMALLOC_DIR");
    println!("cargo::rerun-if-changed={}", mimalloc_source.display());
    let archives: &[&str] = match target_os.as_str() {
        "linux" => &[
            "libJavaScriptCore.a",
            "libWTF.a",
            "libbmalloc.a",
            "libicui18n.a",
            "libicuuc.a",
            "libicudata.a",
        ],
        "macos" => &["libJavaScriptCore.a", "libWTF.a", "libbmalloc.a"],
        "windows" => &[
            "JavaScriptCore.lib",
            "WTF.lib",
            "bmalloc.lib",
            "sicuin.lib",
            "sicuuc.lib",
            "sicudt.lib",
        ],
        other => panic!("unsupported S0 target OS: {other}"),
    };

    if target_os == "linux" {
        println!("cargo::rustc-link-arg=-no-pie");
        println!("cargo::rustc-link-arg=-Wl,--start-group");
    }
    for archive in archives {
        let path = lib_dir.join(archive);
        assert!(path.is_file(), "missing {}", path.display());
        println!("cargo::rustc-link-arg={}", path.display());
    }
    println!("cargo::rustc-link-arg={}", mimalloc_archive.display());
    if target_os == "linux" {
        println!("cargo::rustc-link-arg=-Wl,--end-group");
        for library in ["stdc++", "dl", "pthread", "m"] {
            println!("cargo::rustc-link-lib=dylib={library}");
        }
    } else if target_os == "macos" {
        for library in ["c++", "icucore", "resolv"] {
            println!("cargo::rustc-link-lib=dylib={library}");
        }
    } else {
        for library in [
            "winmm", "bcrypt", "ntdll", "userenv", "dbghelp", "crypt32", "wsock32", "ws2_32",
        ] {
            println!("cargo::rustc-link-lib=dylib={library}");
        }
    }
}
