use kvstore_wal::soak;
use std::time::Instant;

fn main() {
    let mut iterations = 10_000u64;
    let mut start = 0u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match (a.as_str(), args.next().and_then(|v| v.parse::<u64>().ok())) {
            ("--iterations", Some(v)) => iterations = v,
            ("--start-seed", Some(v)) => start = v,
            ("--seed", Some(v)) => {
                // Replay a single seed.
                match soak::run_one(v) {
                    Ok(()) => println!("seed {v}: ok"),
                    Err(e) => {
                        println!("seed {v}: FAIL {e}");
                        std::process::exit(1);
                    }
                }
                return;
            }
            _ => {
                eprintln!("usage: soak [--iterations N] [--start-seed S] | --seed S");
                std::process::exit(2);
            }
        }
    }
    let t = Instant::now();
    let mut failures: Vec<(u64, String)> = Vec::new();
    for seed in start..start + iterations {
        if let Err(e) = soak::run_one(seed) {
            failures.push((seed, e));
        }
    }
    println!(
        "{} runs, {} found a bug ({:.1}s)",
        iterations,
        failures.len(),
        t.elapsed().as_secs_f64()
    );
    for (seed, e) in failures.iter().take(5) {
        println!("  seed {seed}: {e}");
    }
    if !failures.is_empty() {
        std::process::exit(1);
    }
}
