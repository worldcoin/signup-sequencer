fn main() {
    cli_batteries::build_rs().expect("Failed to setup build environment");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=schemas");
    println!("cargo:rerun-if-changed=sol");
}
