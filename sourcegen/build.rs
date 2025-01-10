use std::env;

println!("cargo::rustc-check-cfg=cfg(cross_compile)");

fn main() {
    if env::var("HOST") != env::var("TARGET") {
        println!("cargo:rustc-cfg=cross_compile")
    }
}
