use criterion::Criterion;
use rust_app_template::bench_main;

fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    bench_main(&mut criterion);
    criterion.final_summary();
}
