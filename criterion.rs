fn main() {
    let mut criterion = criterion::Criterion::default().configure_from_args();
    lib::bench::main(&mut criterion);
    criterion.final_summary();
}
