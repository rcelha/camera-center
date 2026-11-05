use cc_core::Mask;

/// Exponential moving average over masks to suppress flicker.
pub struct TemporalSmoother {
    /// Weight of the newest mask, 0..=1.
    alpha: f32,
    state: Option<Vec<f32>>,
    dims: (u32, u32),
}

impl TemporalSmoother {
    pub fn new(alpha: f32) -> Self {
        Self {
            alpha: alpha.clamp(0.05, 1.0),
            state: None,
            dims: (0, 0),
        }
    }

    pub fn reset(&mut self) {
        self.state = None;
    }

    pub fn apply(&mut self, mask: Mask) -> Mask {
        if self.dims != (mask.width, mask.height) {
            self.state = None;
            self.dims = (mask.width, mask.height);
        }
        let state = self
            .state
            .get_or_insert_with(|| mask.data.iter().map(|&v| v as f32).collect());
        let a = self.alpha;
        let data = state
            .iter_mut()
            .zip(&mask.data)
            .map(|(s, &v)| {
                *s += (v as f32 - *s) * a;
                s.round() as u8
            })
            .collect();
        Mask {
            width: mask.width,
            height: mask.height,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converges_towards_new_values() {
        let mut s = TemporalSmoother::new(0.5);
        let first = s.apply(Mask::filled(2, 2, 0));
        assert!(first.data.iter().all(|&v| v == 0));
        let second = s.apply(Mask::filled(2, 2, 200));
        assert!(second.data.iter().all(|&v| v == 100));
        // Size change resets.
        let other = s.apply(Mask::filled(3, 3, 50));
        assert!(other.data.iter().all(|&v| v == 50));
    }
}
