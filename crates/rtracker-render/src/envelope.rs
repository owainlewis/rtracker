use rtracker_core::Envelope;

pub fn apply_envelope(env: &Envelope, buf: &mut [f32]) {
    let n = buf.len() as u64;
    if n == 0 {
        return;
    }
    match *env {
        Envelope::Gate => {}
        Envelope::Ad { attack, decay } => {
            let a = attack.min(n);
            for i in 0..a {
                let g = i as f32 / a.max(1) as f32;
                buf[i as usize] *= g;
            }
            let d_start = a;
            let d_end = (a + decay).min(n);
            let d_len = d_end - d_start;
            for i in 0..d_len {
                let g = 1.0 - (i as f32 / d_len.max(1) as f32);
                buf[(d_start + i) as usize] *= g;
            }
            for i in d_end..n {
                buf[i as usize] = 0.0;
            }
        }
        Envelope::Adsr { attack, decay, sustain, release } => {
            let a = attack.min(n);
            for i in 0..a {
                let g = i as f32 / a.max(1) as f32;
                buf[i as usize] *= g;
            }
            let d_end = (a + decay).min(n);
            for i in a..d_end {
                let t = (i - a) as f32 / decay.max(1) as f32;
                let g = 1.0 + (sustain - 1.0) * t;
                buf[i as usize] *= g;
            }
            let r_start = if n > release { n - release } else { 0 };
            let r_start = r_start.max(d_end);
            for i in d_end..r_start {
                buf[i as usize] *= sustain;
            }
            let r_len = n - r_start;
            for i in 0..r_len {
                let t = i as f32 / r_len.max(1) as f32;
                let g = sustain * (1.0 - t);
                buf[(r_start + i) as usize] *= g;
            }
        }
        Envelope::Exp { attack, tau } => {
            let a = attack.min(n);
            for i in 0..a {
                let g = i as f32 / a.max(1) as f32;
                buf[i as usize] *= g;
            }
            let tau_f = tau.max(1) as f32;
            for i in a..n {
                let t = (i - a) as f32;
                let g = (-t / tau_f).exp();
                buf[i as usize] *= g;
            }
        }
    }
}
