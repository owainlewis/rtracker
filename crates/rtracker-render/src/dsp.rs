use std::f32::consts::PI;

/// Direct Form I biquad. Used by the bandpass noise voice and by future filter FX.
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    pub fn bandpass(sample_rate: f32, center_hz: f32, q: f32) -> Self {
        // RBJ "constant 0 dB peak gain" bandpass.
        let (_, cos_w0, alpha) = Self::rbj_terms(sample_rate, center_hz, q);
        Self::normalize(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha)
    }

    /// RBJ low-pass. `cutoff_hz` is the -3 dB corner; `q` sets resonance.
    pub fn lowpass(sample_rate: f32, cutoff_hz: f32, q: f32) -> Self {
        let (_, cos_w0, alpha) = Self::rbj_terms(sample_rate, cutoff_hz, q);
        let b1 = 1.0 - cos_w0;
        let b0 = b1 / 2.0;
        Self::normalize(b0, b1, b0, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha)
    }

    /// RBJ high-pass. `cutoff_hz` is the -3 dB corner; `q` sets resonance.
    pub fn highpass(sample_rate: f32, cutoff_hz: f32, q: f32) -> Self {
        let (_, cos_w0, alpha) = Self::rbj_terms(sample_rate, cutoff_hz, q);
        let b0 = (1.0 + cos_w0) / 2.0;
        let b1 = -(1.0 + cos_w0);
        Self::normalize(b0, b1, b0, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha)
    }

    /// Shared RBJ intermediate terms: (sin w0, cos w0, alpha). `q` is floored so
    /// a zero/negative Q can't divide by zero.
    fn rbj_terms(sample_rate: f32, freq_hz: f32, q: f32) -> (f32, f32, f32) {
        let w0 = 2.0 * PI * freq_hz / sample_rate;
        let (sin_w0, cos_w0) = w0.sin_cos();
        let alpha = sin_w0 / (2.0 * q.max(1e-4));
        (sin_w0, cos_w0, alpha)
    }

    /// Build a biquad from raw (b0,b1,b2,a0,a1,a2) coefficients, dividing through
    /// by a0 and zeroing the state.
    fn normalize(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}
