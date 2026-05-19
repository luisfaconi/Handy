/// Mix two 16 kHz mono f32 sample buffers.
/// Pads the shorter buffer with zeros. Clamps output to [-1.0, 1.0].
pub fn mix_samples(a: &[f32], b: &[f32]) -> Vec<f32> {
    let len = a.len().max(b.len());
    (0..len)
        .map(|i| {
            let sa = a.get(i).copied().unwrap_or(0.0);
            let sb = b.get(i).copied().unwrap_or(0.0);
            (sa + sb).clamp(-1.0, 1.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_equal_length() {
        let a = vec![0.5, 0.3, -0.1];
        let b = vec![0.2, 0.1, 0.4];
        let out = mix_samples(&a, &b);
        assert_eq!(out.len(), 3);
        assert!((out[0] - 0.7).abs() < 1e-5);
        assert!((out[1] - 0.4).abs() < 1e-5);
        assert!((out[2] - 0.3).abs() < 1e-5);
    }

    #[test]
    fn mix_a_longer_than_b() {
        let a = vec![0.1, 0.2, 0.3, 0.4];
        let b = vec![0.1, 0.1];
        let out = mix_samples(&a, &b);
        assert_eq!(out.len(), 4);
        assert!((out[2] - 0.3).abs() < 1e-5);
        assert!((out[3] - 0.4).abs() < 1e-5);
    }

    #[test]
    fn mix_clamps_over_1() {
        let a = vec![0.8];
        let b = vec![0.8];
        let out = mix_samples(&a, &b);
        assert!((out[0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn mix_clamps_under_neg_1() {
        let a = vec![-0.8];
        let b = vec![-0.8];
        let out = mix_samples(&a, &b);
        assert!((out[0] - (-1.0)).abs() < 1e-5);
    }

    #[test]
    fn mix_empty_inputs_returns_empty() {
        let out = mix_samples(&[], &[]);
        assert!(out.is_empty());
    }

    #[test]
    fn mix_one_empty_returns_other() {
        let a = vec![0.5, 0.3];
        let out = mix_samples(&a, &[]);
        assert_eq!(out, a);
    }
}
