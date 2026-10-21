use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=ui-demo/assets/firenet.ico");
    if !env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("anytofire-icon.res");
    let compiler = env::var_os("LLVM_RC").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("llvm-rc.exe"));
    let status = Command::new(compiler).arg("/no-preprocess").arg("/FO").arg(&out)
        .arg("app.rc").status().expect("could not launch llvm-rc; set LLVM_RC");
    assert!(status.success(), "could not compile Windows icon resource");
    println!("cargo:rustc-link-arg-bin=AnyToFire={}", out.display());
}
