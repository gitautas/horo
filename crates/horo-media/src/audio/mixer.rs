//! Several speakers into one frame for the loudspeaker.

use super::FRAME;

/// Where the soft limiter starts bending: below this, samples pass as summed.
const KNEE: f32 = 0.9;

/// Sum the frames, then limit softly so two loud speakers at once round off
/// instead of clipping hard (which crackles).
pub fn mix<'a>(frames: impl IntoIterator<Item = &'a [f32; FRAME]>, out: &mut [f32; FRAME]) {
    out.fill(0.0);
    for frame in frames {
        for (o, s) in out.iter_mut().zip(frame) {
            *o += s;
        }
    }
    for o in out.iter_mut() {
        *o = limit(*o);
    }
}

/// Identity up to the knee, then an asymptote at 1.0: continuous and
/// monotonic, so loud passages compress rather than distort.
fn limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    let over = (a - KNEE) / (1.0 - KNEE);
    x.signum() * (KNEE + (1.0 - KNEE) * over / (1.0 + over))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_speakers_simply_add() {
        let (a, b) = ([0.2f32; FRAME], [-0.05f32; FRAME]);
        let mut out = [0f32; FRAME];
        mix([&a, &b], &mut out);
        assert!((out[0] - 0.15).abs() < 1e-6);
    }

    #[test]
    fn loud_sums_stay_in_range_and_keep_their_order() {
        let mut last = 0.0;
        for i in 0..400 {
            let x = i as f32 / 100.0; // 0..4
            let y = limit(x);
            assert!(y <= 1.0 && y >= last, "{x} -> {y}");
            assert_eq!(limit(-x), -y);
            last = y;
        }
        // Continuous at the knee.
        assert!((limit(KNEE + 1e-4) - KNEE).abs() < 1e-3);
    }
}
