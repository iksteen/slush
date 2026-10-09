use std::{env, path::PathBuf, process::Command};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let target = env::var("TARGET").unwrap();
    let host = env::var("HOST").unwrap();
    let windows = env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows";
    let macos = env::var("CARGO_CFG_TARGET_OS").unwrap() == "macos";
    let cross = target != host;
    let prefix = manifest.join("target/ghostty").join(&target);
    let source = env::var_os("GHOSTTY_SOURCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("vendor/libghostty-vt"));
    let library = env::var_os("GHOSTTY_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| prefix.join("lib"));
    println!("cargo:rerun-if-env-changed=GHOSTTY_SOURCE_DIR");
    println!("cargo:rerun-if-env-changed=GHOSTTY_LIB_DIR");
    println!("cargo:rerun-if-env-changed=ZIG");
    let include = source.join("include");
    let header = include.join("ghostty/vt.h");
    assert!(
        header.exists(),
        "Ghostty headers not found at {}",
        header.display()
    );
    println!("cargo:rerun-if-changed={}", include.display());
    println!(
        "cargo:rerun-if-changed={}",
        source.join("build.zig.zon").display()
    );
    println!("cargo:rerun-if-changed={}", source.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        source.join("build.zig").display()
    );
    println!("cargo:rerun-if-changed={}", source.join("pkg").display());

    if env::var_os("GHOSTTY_LIB_DIR").is_none() {
        let zon = std::fs::read_to_string(source.join("build.zig.zon")).unwrap();
        let version = zon
            .lines()
            .find_map(|line| line.trim().strip_prefix(".version = \"")?.split('"').next())
            .expect("Ghostty version missing from build.zig.zon");
        let zig = env::var_os("ZIG").unwrap_or_else(|| "zig".into());
        let cache = manifest.join("target/zig-global-cache");
        std::fs::create_dir_all(&cache).unwrap();
        let mut command = Command::new(zig);
        let local_cache = prefix.join("cache");
        command
            .current_dir(&source)
            .env("ZIG_GLOBAL_CACHE_DIR", cache)
            .args([
                "build",
                "-Demit-lib-vt",
                "-Demit-xcframework=false",
                "-Doptimize=ReleaseFast",
                &format!("-Dversion-string={version}"),
            ])
            .arg("--prefix")
            .arg(&prefix)
            .arg("--cache-dir")
            .arg(&local_cache);
        if cross {
            let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
            let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
            let abi = env::var("CARGO_CFG_TARGET_ENV").unwrap();
            let zig_target = if os == "macos" {
                format!("{arch}-macos")
            } else {
                format!("{arch}-{os}-{abi}")
            };
            command.arg(format!("-Dtarget={zig_target}"));
        }
        let status = command
            .status()
            .expect("cannot start Zig; install Zig 0.16 or set ZIG");
        assert!(status.success(), "Ghostty build failed");
    }
    assert!(
        library
            .join(if windows {
                "ghostty-vt-static.lib"
            } else {
                "libghostty-vt.a"
            })
            .exists(),
        "Ghostty static library missing"
    );
    println!(
        "cargo:rustc-env=SLUSH_GHOSTTY_LIB_DIR={}",
        library.display()
    );
    println!(
        "cargo:rustc-env=SLUSH_GHOSTTY_INCLUDE_DIR={}",
        include.display()
    );
    let mut bindings = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .clang_arg(format!("-I{}", include.display()))
        .clang_arg("-DGHOSTTY_STATIC")
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .generate_comments(false)
        .layout_tests(true);
    if cross {
        bindings = bindings.clang_arg(format!("--target={target}"));
    }
    let bindings = bindings
        .generate()
        .expect("cannot generate Ghostty C ABI bindings");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    bindings
        .write_to_file(output.join("ghostty.rs"))
        .expect("cannot write Ghostty bindings");
    println!("cargo:rustc-link-search=native={}", library.display());
    if macos {
        // Darwin has no -Bstatic or -l:filename form. A distinct archive name
        // prevents its linker choosing the colocated Ghostty dylib when rustc
        // links a test executable directly instead of bundling into an rlib.
        std::fs::copy(
            library.join("libghostty-vt.a"),
            output.join("libghostty-vt-static.a"),
        )
        .expect("cannot stage the Ghostty static archive");
        println!("cargo:rustc-link-search=native={}", output.display());
        println!("cargo:rustc-link-lib=static=ghostty-vt-static");
    } else {
        println!(
            "cargo:rustc-link-lib=static:+verbatim={}",
            if windows {
                "ghostty-vt-static.lib"
            } else {
                "libghostty-vt.a"
            }
        );
    }
}
