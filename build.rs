fn main() {
    if cfg!(target_os = "windows") {
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("assets/branding/monster-head-v2.ico");
        resource
            .compile()
            .expect("failed to embed application icon");
    }
    println!("cargo:rerun-if-changed=assets/branding/monster-head-v2.ico");
}
