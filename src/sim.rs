//! A simulated disk for crash testing.
//!
//! Appended bytes land in a `pending` buffer (the OS page cache). `sync` moves
//! them to `durable`. A simulated power loss keeps `durable` plus a random
//! prefix of `pending`, optionally with damaged trailing bytes, which is what a
//! torn write looks like.

use crate::storage::Storage;
use std::cell::RefCell;
use std::io;
use std::rc::Rc;

/// xorshift64*; enough for test scheduling and keeps the crate dependency free.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `0..n` (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

struct Inner {
    durable: Vec<u8>,
    pending: Vec<u8>,
    dead: bool,
    /// Storage calls remaining before the disk dies (power loss).
    crash_in: Option<usize>,
    /// Storage calls remaining before one call fails but the disk stays up.
    fail_in: Option<usize>,
    rng: Rng,
}

enum Fault {
    None,
    Crash,
    Fail,
}

impl Inner {
    fn tick(&mut self) -> io::Result<Fault> {
        if self.dead {
            return Err(io::Error::other("disk is dead"));
        }
        for (slot, fault) in [
            (&mut self.crash_in, Fault::Crash),
            (&mut self.fail_in, Fault::Fail),
        ] {
            if let Some(n) = slot {
                if *n == 0 {
                    *slot = None;
                    return Ok(fault);
                }
                *n -= 1;
            }
        }
        Ok(Fault::None)
    }
}

#[derive(Clone)]
pub struct SimDisk(Rc<RefCell<Inner>>);

impl SimDisk {
    pub fn new(seed: u64) -> Self {
        SimDisk(Rc::new(RefCell::new(Inner {
            durable: Vec::new(),
            pending: Vec::new(),
            dead: false,
            crash_in: None,
            fail_in: None,
            rng: Rng::new(seed),
        })))
    }

    pub fn crash_in(&self, calls: usize) {
        self.0.borrow_mut().crash_in = Some(calls);
    }

    pub fn fail_in(&self, calls: usize) {
        self.0.borrow_mut().fail_in = Some(calls);
    }

    pub fn is_dead(&self) -> bool {
        self.0.borrow().dead
    }

    /// Bytes that survive a power loss right now.
    pub fn durable_len(&self) -> usize {
        self.0.borrow().durable.len()
    }

    /// Power loss and restart: unsynced bytes mostly vanish. Brings the disk
    /// back up with no armed faults.
    pub fn power_cycle(&self) {
        let mut g = self.0.borrow_mut();
        let keep = if g.pending.is_empty() {
            0
        } else {
            let n = g.pending.len() + 1;
            g.rng.below(n).min(g.pending.len())
        };
        let mut kept: Vec<u8> = g.pending[..keep].to_vec();
        if !kept.is_empty() && g.rng.chance(30) {
            // Torn sector: the tail of what was kept is garbage.
            let bad = 1 + g.rng.below(kept.len().min(8));
            let at = kept.len() - bad;
            for b in &mut kept[at..] {
                *b = g.rng.next() as u8;
            }
        }
        g.durable.extend_from_slice(&kept);
        g.pending.clear();
        g.dead = false;
        g.crash_in = None;
        g.fail_in = None;
    }

    /// Raw contents a reader would see, for assertions.
    pub fn visible(&self) -> Vec<u8> {
        let g = self.0.borrow();
        let mut v = g.durable.clone();
        v.extend_from_slice(&g.pending);
        v
    }
}

impl Storage for SimDisk {
    fn read_all(&mut self) -> io::Result<Vec<u8>> {
        let g = self.0.borrow();
        if g.dead {
            return Err(io::Error::other("disk is dead"));
        }
        drop(g);
        Ok(self.visible())
    }

    fn append(&mut self, data: &[u8]) -> io::Result<()> {
        let mut g = self.0.borrow_mut();
        match g.tick()? {
            Fault::None => {
                g.pending.extend_from_slice(data);
                Ok(())
            }
            f => {
                let n = g.rng.below(data.len());
                g.pending.extend_from_slice(&data[..n]);
                if matches!(f, Fault::Crash) {
                    g.dead = true;
                    Err(io::Error::other("simulated crash"))
                } else {
                    Err(io::Error::other("simulated write error"))
                }
            }
        }
    }

    fn sync(&mut self) -> io::Result<()> {
        let mut g = self.0.borrow_mut();
        match g.tick()? {
            Fault::None => {
                let p = std::mem::take(&mut g.pending);
                g.durable.extend_from_slice(&p);
                Ok(())
            }
            Fault::Crash => {
                g.dead = true;
                Err(io::Error::other("simulated crash"))
            }
            Fault::Fail => Err(io::Error::other("simulated sync error")),
        }
    }

    fn truncate(&mut self, len: u64) -> io::Result<()> {
        let mut g = self.0.borrow_mut();
        match g.tick()? {
            Fault::None => {
                let len = len as usize;
                let dlen = g.durable.len();
                if len <= dlen {
                    g.durable.truncate(len);
                    g.pending.clear();
                } else {
                    g.pending.truncate(len - dlen);
                }
                Ok(())
            }
            Fault::Crash => {
                g.dead = true;
                Err(io::Error::other("simulated crash"))
            }
            Fault::Fail => Err(io::Error::other("simulated truncate error")),
        }
    }

    fn replace(&mut self, data: &[u8]) -> io::Result<()> {
        let mut g = self.0.borrow_mut();
        match g.tick()? {
            Fault::None => {
                g.durable = data.to_vec();
                g.pending.clear();
                Ok(())
            }
            Fault::Crash => {
                // rename is atomic: the crash lands before or after it.
                if g.rng.chance(50) {
                    g.durable = data.to_vec();
                    g.pending.clear();
                }
                g.dead = true;
                Err(io::Error::other("simulated crash"))
            }
            Fault::Fail => Err(io::Error::other("simulated replace error")),
        }
    }
}
