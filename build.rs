fn main() {
    slint_build::compile("ui/main.slint").unwrap();
    println!("cargo:rerun-if-changed=ui/main.slint");
    println!("cargo:rerun-if-changed=ui/sidebar.slint");
    println!("cargo:rerun-if-changed=ui/top_player.slint");
    println!("cargo:rerun-if-changed=ui/queue_panel.slint");
    println!("cargo:rerun-if-changed=ui/types.slint");
    println!("cargo:rerun-if-changed=ui/Theme.slint");
    println!("cargo:rerun-if-changed=ui/assets/icons");
}
