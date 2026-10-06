//! Times the Linux process and cgroup scans: `cargo run --release -p nysm-collect --example bench_scan`
use std::time::Instant;

fn main() {
    let mut p = nysm_collect::native();
    for (name, f) in [
        (
            "processes",
            &mut (|p: &mut Box<dyn nysm_collect::Platform>| {
                p.processes().map(|s| s.processes.len())
            })
                as &mut dyn FnMut(
                    &mut Box<dyn nysm_collect::Platform>,
                ) -> nysm_collect::CResult<usize>,
        ),
        ("cgroups", &mut |p: &mut Box<dyn nysm_collect::Platform>| {
            p.cgroups().map(|s| s.groups.len())
        }),
    ] {
        let n = 20;
        let t = Instant::now();
        let mut count = 0;
        for _ in 0..n {
            count = f(&mut p).unwrap_or(0);
        }
        let per = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("{name:10} {count:5} items  {per:6.2} ms per scan");
    }
}
