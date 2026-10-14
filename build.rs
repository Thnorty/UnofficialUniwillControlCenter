fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");
    // The icon and the manifest: administrator rights (the EC is reached through the BIOS WMI interface), per-monitor
    // DPI awareness and Windows 10/11 compatibility. UUCC_NO_ADMIN builds a copy that runs without
    // administrator rights, for trying the window with --demo.
    println!("cargo:rerun-if-env-changed=UUCC_NO_ADMIN");
    let resources = if std::env::var_os("UUCC_NO_ADMIN").is_some() { "res/app-demo.rc" } else { "res/app.rc" };
    embed_resource::compile(resources, embed_resource::NONE).manifest_required().unwrap();
    println!("cargo:rerun-if-changed=res");
}
