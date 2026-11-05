fn main() {
    tauri_build::build();

    // whisper-rs (feature "metal") no macOS:
    // 1) -ObjC força o carregamento dos objetos Objective-C do ggml-metal
    //    (ggml-metal-device.m), que só são referenciados via runtime ObjC.
    // 2) Esse arquivo usa checagens de disponibilidade (___isPlatformVersionAtLeast)
    //    providas pelo compiler-rt do clang; o rustc linka com -nodefaultlibs e não
    //    as inclui, então linkamos libclang_rt.osx explicitamente.
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-arg=-ObjC");
        if let Ok(out) = std::process::Command::new("clang")
            .arg("-print-resource-dir")
            .output()
        {
            let resource = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let libdir = format!("{resource}/lib/darwin");
            if std::path::Path::new(&libdir).join("libclang_rt.osx.a").exists() {
                println!("cargo:rustc-link-search=native={libdir}");
                println!("cargo:rustc-link-lib=static=clang_rt.osx");
            } else {
                eprintln!(
                    "[fala:build] aviso: libclang_rt.osx.a não encontrada em {libdir}; \
                     se o link falhar com ___isPlatformVersionAtLeast, instale o Xcode CLT completo"
                );
            }
        }
    }
}
