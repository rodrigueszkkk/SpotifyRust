fn main() {
    let mut config = slint_build::CompilerConfiguration::new();
    config = config.with_style("cupertino-dark".into());
    slint_build::compile_with_config("ui/main.slint", config).unwrap();
    println!("cargo:rerun-if-changed=ui/main.slint");
    println!("cargo:rerun-if-changed=ui/sidebar.slint");
    println!("cargo:rerun-if-changed=ui/top_player.slint");
    println!("cargo:rerun-if-changed=ui/queue_panel.slint");
    println!("cargo:rerun-if-changed=ui/types.slint");
    println!("cargo:rerun-if-changed=ui/Theme.slint");
    println!("cargo:rerun-if-changed=ui/assets/icons");
}
