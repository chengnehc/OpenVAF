use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt::Display;
use std::path::Path;

use target::spec::supported_targets;
use xshell::{cmd, Shell};

/// Reads an environment variable and adds it to dependencies.
/// Supposed to be used for all variables except those set for build scripts by cargo
/// <https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-build-scripts>
fn tracked_env_var_os<K: AsRef<OsStr> + Display>(key: K) -> Option<OsString> {
    // println!("cargo:rerun-if-env-changed={}", key);
    env::var_os(key)
}

fn main() {
    let osdi_dir = env::current_dir().unwrap();
    let src_file = osdi_dir.join("stdlib.c");

    let sh = Shell::new().unwrap();

    for file in sh.read_dir("header").unwrap() {
        if file.extension().is_none_or(|ext| ext != "h") {
            continue;
        }
        if let Some(name) = file.file_stem().and_then(|name| name.to_str()) {
            let def_name = name.to_uppercase();
            if let Some(version_str) = name.strip_prefix("osdi_") {
                let out_dir = env::var_os("OUT_DIR").unwrap();
                println!("cargo:rerun-if-changed={}", file.display());

                for target in supported_targets() {
                    let target_name = &target.llvm_target;
                    let out_file =
                        Path::new(&out_dir).join(format!("stdlib_{version_str}_{target_name}.bc"));

                    if tracked_env_var_os("RUST_CHECK").is_some() {
                        // If we're just running `cargo check`, there's no need to actually
                        // compile the `stdlib`
                        sh.write_file(out_file, []).expect("failed to write dummy file");
                    } else {
                        let mut cmd = cmd!(sh, "clang -emit-llvm -O3 -D{def_name} -DNO_STD -o {out_file} -c {src_file} -target {target_name}");
                        if !target.options.is_like_windows {
                            cmd = cmd.arg("-fPIC");
                        }
                        cmd.run().expect("failed to generate bitcode");
                    }
                }
            }
        }
    }

    println!("cargo:rerun-if-changed={}", src_file.display());
}
