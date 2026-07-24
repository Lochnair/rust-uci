// Copyright (c) 2021-2022,2024-2025 Benjamin Ludewig, Hugo Hakim Damer and the
// other rust-uci contributors.
// SPDX-License-Identifier: MIT OR Apache-2.0

extern crate bindgen;
extern crate cmake;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::{env, fs};

fn compiler_output(compiler: &cc::Tool, argument: &str) -> Option<String> {
    let mut command = compiler.to_command();
    command.arg(argument);

    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn configure_bindgen_target(mut builder: bindgen::Builder) -> bindgen::Builder {
    let host = env::var("HOST").expect("Cargo did not set HOST");
    let target = env::var("TARGET").expect("Cargo did not set TARGET");

    if host == target {
        return builder;
    }

    let compiler = cc::Build::new().target(&target).get_compiler();
    let bindgen_target = env::var("BINDGEN_TARGET")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| compiler_output(&compiler, "-dumpmachine"))
        .unwrap_or(target);
    builder = builder.clang_arg(format!("--target={bindgen_target}"));

    if let Some(sysroot) = compiler_output(&compiler, "-print-sysroot") {
        builder = builder.clang_arg(format!("--sysroot={sysroot}"));
    }

    builder
}

fn cargo_target_uses_static_crt() -> bool {
    env::var("CARGO_CFG_TARGET_FEATURE")
        .unwrap_or_default()
        .split(',')
        .any(|feature| feature == "crt-static")
}

fn append_encoded_rustflags(command: &mut Command, encoded: OsString) {
    for flag in encoded
        .to_string_lossy()
        .split('\x1f')
        .filter(|flag| !flag.is_empty())
    {
        command.arg(flag);
    }
}

fn rustc_target_cfg() -> Option<String> {
    let rustc = env::var_os("RUSTC")?;
    let target = env::var_os("TARGET")?;
    let mut command = Command::new(rustc);
    command.arg("--print=cfg").arg("--target").arg(target);

    if let Some(encoded) = env::var_os("CARGO_ENCODED_RUSTFLAGS") {
        append_encoded_rustflags(&mut command, encoded);
    }

    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout).ok()
}

fn target_uses_static_crt() -> bool {
    if cargo_target_uses_static_crt() {
        return true;
    }

    rustc_target_cfg()
        .map(|cfg| {
            cfg.lines()
                .any(|line| line == r#"target_feature="crt-static""#)
        })
        .unwrap_or(false)
}

fn main() {
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    let vendored_build = env::var_os("CARGO_FEATURE_VENDORED").is_some();
    let static_link = vendored_build && target_uses_static_crt();

    // don't run cmake if running for docs.rs
    if env::var("DOCS_RS").is_ok() {
        fs::copy("generated/bindings.rs", out_path).unwrap();
        return;
    }

    let mut builder = configure_bindgen_target(bindgen::Builder::default());

    match vendored_build {
        false => {
            // if UCI_DIR is present, use it to look for the header file and precompiled libs
            if let Ok(uci_dir) = env::var("UCI_DIR") {
                println!("cargo:rustc-link-search=native={}/lib", uci_dir);
                builder = builder.clang_arg(format!("-I{}/include", uci_dir));
            } else {
                panic!(
                    "vendored is disabled, but UCI_DIR is not set; refusing to build \
             vendored libuci/libubox. Set UCI_DIR to the libuci prefix (with include/ and lib/), \
             or enable the 'vendored' feature."
                );
            }
        }
        true => {
            let ubox_target = if static_link { "ubox-static" } else { "ubox" };
            let uci_target = if static_link { "uci-static" } else { "uci" };
            let build_static = if static_link { "ON" } else { "OFF" };
            let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
            let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

            let json_c_out = out_dir.join("json-c");
            let json_c = cmake::Config::new(manifest_dir.join("json-c"))
                .out_dir(&json_c_out)
                .define("BUILD_SHARED_LIBS", "OFF")
                .define("BUILD_STATIC_LIBS", "ON")
                .define("BUILD_TESTING", "OFF")
                .define("BUILD_APPS", "OFF")
                .define("DISABLE_WERROR", "ON")
                .define("DISABLE_EXTRA_LIBS", "ON")
                .define("CMAKE_INSTALL_LIBDIR", "lib")
                .define("CMAKE_INSTALL_INCLUDEDIR", "include")
                .build();

            let json_c_library_dir = json_c.join("lib");
            let json_c_pkgconfig_dir = json_c_library_dir.join("pkgconfig");
            let libubox_source = manifest_dir.join("libubox");
            let libubox_out = out_dir.join("libubox");
            let libubox_build = libubox_out.join("build");
            cmake::Config::new(&libubox_source)
                .out_dir(&libubox_out)
                .define("BUILD_LUA", "OFF")
                .define("BUILD_EXAMPLES", "OFF")
                .build_target(ubox_target)
                .env("PKG_CONFIG_PATH", "")
                .env("PKG_CONFIG_LIBDIR", &json_c_pkgconfig_dir)
                .env("PKG_CONFIG_SYSROOT_DIR", "")
                .define("CMAKE_LIBRARY_PATH", &json_c_library_dir)
                .env("CMAKE_POLICY_VERSION_MINIMUM", "3.5")
                .build();

            let libuci_source = manifest_dir.join("uci");
            let libuci_out = out_dir.join("libuci");
            let libuci_build = libuci_out.join("build");
            cmake::Config::new(&libuci_source)
                .out_dir(&libuci_out)
                .define("BUILD_LUA", "OFF")
                .define("BUILD_STATIC", build_static)
                .build_target(uci_target)
                .define("ubox_include_dir", &manifest_dir)
                .define("CMAKE_LIBRARY_PATH", &libubox_build)
                .build();

            println!("cargo:rustc-link-search=native={}", libubox_build.display());
            println!("cargo:rustc-link-search=native={}", libuci_build.display());

            builder = builder.clang_arg(format!("-I{}", libuci_source.display()));
        }
    }

    // Link to libuci and libubox
    let link_kind = if static_link { "static" } else { "dylib" };
    println!("cargo:rustc-link-lib={link_kind}=uci");
    println!("cargo:rustc-link-lib={link_kind}=ubox");

    // Tell cargo to invalidate the built crate whenever the wrapper changes
    println!("cargo:rerun-if-changed=wrapper.h");

    let bindings = builder
        .header("wrapper.h")
        // Tell cargo to invalidate the built crate whenever any of the
        // included header files changed.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .allowlist_function("uci_.*")
        .allowlist_type("uci_.*")
        .allowlist_var("uci_.*")
        .allowlist_var("UCI_.*")
        .no_debug("uci_ptr")
        .generate()
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    bindings
        .write_to_file(out_path)
        .expect("Couldn't write bindings!");
}
