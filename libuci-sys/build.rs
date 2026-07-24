// Copyright (c) 2021-2022,2024-2025 Benjamin Ludewig, Hugo Hakim Damer and the
// other rust-uci contributors.
// SPDX-License-Identifier: MIT OR Apache-2.0

extern crate bindgen;
extern crate cmake;

use std::path::PathBuf;
use std::{env, fs};

fn configure_bindgen_target(mut builder: bindgen::Builder) -> bindgen::Builder {
    let host = env::var("HOST").expect("Cargo did not set HOST");
    let cargo_target = env::var("TARGET").expect("Cargo did not set TARGET");

    let bindgen_target = env::var("BINDGEN_TARGET").unwrap_or_else(|_| cargo_target.clone());

    if bindgen_target == host {
        return builder;
    }

    builder = builder.clang_arg(format!("--target={bindgen_target}"));

    let compiler = cc::Build::new().target(&cargo_target).get_compiler();

    let mut command = compiler.to_command();
    command.arg("-print-sysroot");

    match command.output() {
        Ok(output) if output.status.success() => {
            let sysroot = String::from_utf8_lossy(&output.stdout);
            let sysroot = sysroot.trim();

            if !sysroot.is_empty() {
                builder = builder.clang_arg(format!("--sysroot={sysroot}"));
            }
        }
        _ => {
            // Some toolchains do not expose a sysroot this way.
            // Bindgen may still work, or the caller can provide
            // BINDGEN_EXTRA_CLANG_ARGS_<TARGET>.
        }
    }

    builder
}

fn main() {
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    let vendored_build = env::var_os("CARGO_FEATURE_VENDORED").is_some();

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
            // otherwise build libuci & libubox from source
            let libubox = cmake::Config::new("libubox")
                .define("BUILD_LUA", "OFF")
                .define("BUILD_EXAMPLES", "OFF")
                // Required to build with newer CMake versions (e.g., on Arch Linux)
                .env("CMAKE_POLICY_VERSION_MINIMUM", "3.5")
                .build();
            let libuci = cmake::Config::new("uci")
                .define("BUILD_LUA", "OFF")
                .define("BUILD_STATIC", "OFF")
                .define(
                    "ubox_include_dir",
                    libubox.join("include").as_path().display().to_string(),
                )
                .build();
            println!("cargo:rustc-link-search=native={}/lib", libuci.display());
            builder = builder.clang_arg(format!("-I{}/include", libuci.display()))
        }
    }

    // Link to libuci and libubox
    println!("cargo:rustc-link-lib=dylib=uci");
    println!("cargo:rustc-link-lib=dylib=ubox");

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
