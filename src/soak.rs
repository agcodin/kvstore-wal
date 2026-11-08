//! Randomized workload with injected crashes and I/O errors.
//!
//! One run drives a store on a [`SimDisk`], mirrors every acknowledged write in
//! a model, and checks after each failure and each simulated power loss that
//! the store holds exactly what the model says it should.
//!
//! Contract being checked, with `SyncPolicy::Always`:
//!  * a write that returned `Ok` is never lost, across any number of crashes;
//!  * a write that returned `Err` without a crash has no effect, in memory or
//!    after the next restart;
//!  * a write interrupted by a crash may or may not be present, but nothing
//!    else may differ from the model.

use crate::sim::{Rng, SimDisk};
use crate::store::{Options, Store, SyncPolicy};
use std::collections::BTreeMap;

type Model = BTreeMap<Vec<u8>, Vec<u8>>;

#[derive(Clone, Debug)]
enum Step {
    Put(Vec<u8>, Vec<u8>),
    Del(Vec<u8>),
}

fn apply_model(m: &mut Model, s: &Step) {
    match s {
        Step::Put(k, v) => {
            m.insert(k.clone(), v.clone());
        }
        Step::Del(k) => {
            m.remove(k);
        }
    }
}

fn dump(s: &Store<SimDisk>) -> Model {
    s.scan_prefix(b"")
        .map(|(k, v)| (k.to_vec(), v.to_vec()))
        .collect()
}

fn opts() -> Options {
    Options {
        sync: SyncPolicy::Always,
        // Small, so compaction runs often inside short workloads.
        auto_compact_min_bytes: Some(300),
    }
}

/// Run one randomized scenario. `Err` describes the first invariant violation.
pub fn run_one(seed: u64) -> Result<(), String> {
    let mut rng = Rng::new(seed);
    let disk = SimDisk::new(seed ^ 0xABCD);
    let mut store =
        Store::open_with(disk.clone(), opts()).map_err(|e| format!("initial open: {e}"))?;
    let mut model = Model::new();
    let steps = 20 + rng.below(180);

    for i in 0..steps {
        if rng.chance(8) {
            if rng.chance(50) {
                disk.crash_in(rng.below(6));
            } else {
                disk.fail_in(rng.below(6));
            }
        }
        let key = format!("{}{}", ["a/", "b/", "c"][rng.below(3)], rng.below(12)).into_bytes();
        let step = if rng.chance(25) {
            Step::Del(key)
        } else {
            let vlen = rng.below(40);
            let v: Vec<u8> = (0..vlen).map(|_| rng.next() as u8).collect();
            Step::Put(key, v)
        };
        let res = match &step {
            Step::Put(k, v) => store.put(k, v).map(|_| ()),
            Step::Del(k) => store.delete(k).map(|_| ()),
        };
        match res {
            Ok(()) => apply_model(&mut model, &step),
            Err(_) if disk.is_dead() => {
                // Crash during the write. Recover and accept either outcome.
                let mut with = model.clone();
                apply_model(&mut with, &step);
                drop(store);
                disk.power_cycle();
                store = Store::open_with(disk.clone(), opts())
                    .map_err(|e| format!("step {i}: reopen after crash: {e}"))?;
                let got = dump(&store);
                if std::env::var("SOAK_DEBUG").is_ok() {
                    eprintln!("got={got:?}\nmodel={model:?}\nwith={with:?}");
                }
                if got == with {
                    model = with;
                } else if got != model {
                    return Err(format!(
                        "step {i}: after crash during {step:?}, store has {} keys; \
                         expected {} (write lost) or {} (write kept)",
                        got.len(),
                        model.len(),
                        with.len()
                    ));
                }
            }
            Err(_) => {
                // Transient error: the write must have had no effect.
                let got = dump(&store);
                if got != model {
                    return Err(format!("step {i}: failed {step:?} changed in-memory state"));
                }
            }
        }

        if rng.chance(5) {
            let _ = store.compact();
        }

        // Idle power loss. Everything acknowledged was synced, so nothing may
        // be missing afterwards.
        if rng.chance(6) {
            drop(store);
            disk.power_cycle();
            store = Store::open_with(disk.clone(), opts())
                .map_err(|e| format!("step {i}: reopen after power loss: {e}"))?;
            let got = dump(&store);
            if got != model {
                return Err(format!(
                    "step {i}: acknowledged data lost after power loss: store has {} keys, \
                     model has {}",
                    got.len(),
                    model.len()
                ));
            }
        }
    }

    drop(store);
    disk.power_cycle();
    let store = Store::open_with(disk, opts()).map_err(|e| format!("final open: {e}"))?;
    let got = dump(&store);
    if got != model {
        return Err(format!(
            "final: acknowledged data lost: store has {} keys, model has {}",
            got.len(),
            model.len()
        ));
    }
    Ok(())
}
