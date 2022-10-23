use std::time::Instant;

use jujutsu_lib::git;

use crate::ui::Ui;

pub struct Progress<'a> {
    ui: &'a mut Ui,
    rate: RateEstimate,
    buffer: String,
    printed: bool,
}

impl<'a> Progress<'a> {
    pub fn new(ui: &'a mut Ui) -> Self {
        Self {
            ui,
            rate: RateEstimate::new(),
            buffer: String::new(),
            printed: false,
        }
    }

    pub fn update(&mut self, now: Instant, progress: &git::Progress) {
        use std::fmt::Write as _;

        const CLEAR_TRAILING: &str = "\x1b[K";
        self.buffer.clear();
        write!(
            self.buffer,
            "\r{}{: >3.0}%",
            CLEAR_TRAILING,
            100.0 * progress.overall
        )
        .unwrap();
        if let Some(estimate) = progress
            .bytes_downloaded
            .and_then(|x| self.rate.update(now, x))
        {
            let (scaled, prefix) = binary_prefix(estimate);
            write!(self.buffer, " at {: >5.1} {}B/s", scaled, prefix).unwrap();
        }
        _ = write!(self.ui, "{}", self.buffer);
        self.printed = true;
    }
}

impl Drop for Progress<'_> {
    fn drop(&mut self) {
        if self.printed {
            let _ = writeln!(self.ui);
        }
    }
}

/// Find the smallest binary prefix with which the whole part of `x` is at most
/// three digits, and return the scaled `x` and that prefix.
fn binary_prefix(x: f32) -> (f32, &'static str) {
    const TABLE: [&str; 9] = ["", "Ki", "Mi", "Gi", "Ti", "Pi", "Ei", "Zi", "Yi"];

    let mut i = 0;
    let mut scaled = x;
    while scaled.abs() >= 1000.0 && i < TABLE.len() - 1 {
        i += 1;
        scaled /= 1024.0;
    }
    (scaled, TABLE[i])
}

struct RateEstimate {
    state: Option<RateEstimateState>,
}

impl RateEstimate {
    fn new() -> Self {
        RateEstimate { state: None }
    }

    /// Compute smoothed rate from an update
    fn update(&mut self, now: Instant, total: u64) -> Option<f32> {
        if let Some(ref mut state) = self.state {
            return Some(state.update(now, total));
        }

        self.state = Some(RateEstimateState {
            total,
            avg_rate: None,
            last_sample: now,
        });
        None
    }
}

struct RateEstimateState {
    total: u64,
    avg_rate: Option<f32>,
    last_sample: Instant,
}

impl RateEstimateState {
    fn update(&mut self, now: Instant, total: u64) -> f32 {
        let delta = total - self.total;
        self.total = total;
        let dt = now - self.last_sample;
        self.last_sample = now;
        let sample = delta as f32 / dt.as_secs_f32();
        match self.avg_rate {
            None => *self.avg_rate.insert(sample),
            Some(ref mut avg_rate) => {
                // From Algorithms for Unevenly Spaced Time Series: Moving
                // Averages and Other Rolling Operators (Andreas Eckner, 2019)
                const TIME_WINDOW: f32 = 2.0;
                let alpha = 1.0 - (-dt.as_secs_f32() / TIME_WINDOW).exp();
                *avg_rate += alpha * (sample - *avg_rate);
                *avg_rate
            }
        }
    }
}
