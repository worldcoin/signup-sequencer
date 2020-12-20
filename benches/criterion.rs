use criterion::Criterion;
use rust_app_template::bench;

fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    bench::main(&mut criterion);
    criterion.final_summary();
}
