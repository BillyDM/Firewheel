//! Miscellaneous DSP algorithms

/// Detects the maximum absolute peak value in a buffer of samples.
pub fn max_peak(data: &[f32]) -> f32 {
    const CHUNK: usize = 8;

    // Processing in chunks like this breaks the dependency chain which allows
    // the compiler to properly autovectorize this loop.
    let mut tmp = [0.0; CHUNK];
    for x in data.chunks_exact(CHUNK) {
        for i in 0..CHUNK {
            let abs = x[i].abs();
            if abs > tmp[i] {
                tmp[i] = abs;
            }
        }
    }

    let mut res = 0.0;
    for s in tmp {
        if s > res {
            res = s;
        }
    }
    for &s in &data[(data.len() / CHUNK) * CHUNK..] {
        let abs = s.abs();
        if abs > res {
            res = abs;
        }
    }
    res
}
