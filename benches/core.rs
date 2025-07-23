use criterion::{criterion_group, criterion_main, Criterion};
use firewheel::diff::{Diff, Patch, PathBuilder};
use std::hint::black_box;

/// A simple XOR-based RNG.
pub struct XorRng {
    state: u32,
}

impl XorRng {
    /// Create a new [XorRng].
    pub const fn new() -> Self {
        Self { state: 1 }
    }

    /// Generate a new random number, updating the RNG's internal state.
    pub fn generate(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;

        self.state
    }

    /// Produce a random float in the range [0..1).
    pub fn rand_unit(&mut self) -> f32 {
        let value = self.generate() as f32;
        value / u32::MAX as f32
    }
}

#[derive(Diff, Patch, Default, Clone)]
struct ShallowParams {
    a: f32,
    b: f32,
    c: bool,
}

impl ShallowParams {
    fn randomize(&mut self, rng: &mut XorRng) {
        self.a = rng.rand_unit();
        self.b = rng.rand_unit();
        self.c = (rng.generate() & 1) > 0;
    }
}

#[derive(Diff, Patch, Default, Clone)]
struct SingleNesting {
    a: ShallowParams,
    b: bool,
}

#[derive(Diff, Patch, Default, Clone)]
struct DoubleNesting {
    a: SingleNesting,
    b: bool,
}

#[derive(Diff, Patch, Default, Clone)]
struct TripleNesting {
    a: DoubleNesting,
    b: bool,
}

/// This struct will force additional path allocation.
#[derive(Diff, Patch, Default, Clone)]
struct QuadNesting {
    a: TripleNesting,
    b: bool,
}

pub fn criterion_benchmark(c: &mut Criterion) {
    let mut rng = XorRng::new();
    let mut sources = vec![ShallowParams::default(); 128];
    for source in &mut sources {
        source.randomize(&mut rng);
    }

    let target = ShallowParams::default();
    let mut messages = Vec::with_capacity(128 * 3);

    c.bench_function("diffing shallow", |b| {
        b.iter(|| {
            for source in &sources {
                source.diff(&target, PathBuilder::default(), &mut messages);
            }

            black_box(&messages);
            messages.clear();
        })
    });

    let mut rng = XorRng::new();
    let mut sources = vec![ShallowParams::default(); 128];
    for source in &mut sources {
        source.randomize(&mut rng);
    }

    let mut target = ShallowParams::default();
    let mut messages = Vec::with_capacity(128 * 3);
    for source in &sources {
        source.diff(&target, PathBuilder::default(), &mut messages);
    }

    c.bench_function("patching shallow", |b| {
        b.iter(|| {
            for message in messages.drain(..) {
                target.apply(ShallowParams::patch_event(message).unwrap());
                black_box(&target);
            }
        })
    });

    let mut rng = XorRng::new();
    let mut sources = vec![DoubleNesting::default(); 128];
    for source in &mut sources {
        source.a.a.randomize(&mut rng);
    }

    let mut target = DoubleNesting::default();
    let mut messages = Vec::with_capacity(128 * 3);
    for source in &sources {
        source.diff(&target, PathBuilder::default(), &mut messages);
    }

    c.bench_function("patching three", |b| {
        b.iter(|| {
            for message in messages.drain(..) {
                target.apply(DoubleNesting::patch_event(message).unwrap());
                black_box(&target);
            }
        })
    });

    let mut rng = XorRng::new();
    let mut sources = vec![ShallowParams::default(); 128];

    for source in &mut sources {
        source.randomize(&mut rng);
    }

    let mut target = ShallowParams::default();
    let mut messages = Vec::with_capacity(128 * 3);

    c.bench_function("end-to-end shallow", |b| {
        b.iter(|| {
            for source in &sources {
                source.diff(&target, PathBuilder::default(), &mut messages);
            }

            for message in messages.drain(..) {
                target.apply(ShallowParams::patch_event(message).unwrap());
                black_box(&target);
            }
        })
    });

    let mut rng = XorRng::new();
    let mut sources = vec![DoubleNesting::default(); 128];
    for source in &mut sources {
        source.a.a.randomize(&mut rng);
    }

    let mut target = DoubleNesting::default();
    let mut messages = Vec::with_capacity(128 * 3);

    c.bench_function("end-to-end three", |b| {
        b.iter(|| {
            for source in &sources {
                source.diff(&target, PathBuilder::default(), &mut messages);
            }

            for message in messages.drain(..) {
                target.apply(DoubleNesting::patch_event(message).unwrap());
                black_box(&target);
            }
        })
    });

    let mut rng = XorRng::new();
    let mut sources = vec![QuadNesting::default(); 128];
    for source in &mut sources {
        source.a.a.a.a.randomize(&mut rng);
    }

    let mut target = QuadNesting::default();
    let mut messages = Vec::with_capacity(128 * 3);

    c.bench_function("end-to-end five", |b| {
        b.iter(|| {
            for source in &sources {
                source.diff(&target, PathBuilder::default(), &mut messages);
            }

            for message in messages.drain(..) {
                target.apply(QuadNesting::patch_event(message).unwrap());
                black_box(&target);
            }
        })
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
