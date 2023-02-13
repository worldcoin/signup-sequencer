fn main() {
    cli_batteries::build_rs().unwrap();

    println!("cargo:rerun-if-changed=schemas");
}
