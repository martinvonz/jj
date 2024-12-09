use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;
use jj_lib::fmt_util::binary_prefix;
use jj_lib::git;
use jj_lib::repo_path::RepoPath;

use crate::cleanup_guard::CleanupGuard;
use crate::text_util;
use crate::ui::OutputGuard;
use crate::ui::ProgressOutput;
use crate::ui::Ui;

pub struct Progress {
    next_print: Instant,
    rate: RateEstimate,
    buffer: String,
    guard: Option<CleanupGuard>,
}

impl Progress {
    pub fn new(now: Instant) -> Self {
        Self {
            next_print: now + INITIAL_DELAY,
            rate: RateEstimate::new(),
            buffer: String::new(),
            guard: None,
        }
    }

    pub fn update<W: std::io::Write>(
        &mut self,
        now: Instant,
        progress: &git::Progress,
        output: &mut ProgressOutput<W>,
    ) -> io::Result<()> {
        use std::fmt::Write as _;

        if progress.overall == 1.0 {
            write!(output, "\r{}", Clear(ClearType::CurrentLine))?;
            output.flush()?;
            return Ok(());
        }

        let rate = progress
            .bytes_downloaded
            .and_then(|x| self.rate.update(now, x));
        if now < self.next_print {
            return Ok(());
        }
        self.next_print = now + Duration::from_secs(1) / UPDATE_HZ;
        if self.guard.is_none() {
            let guard = output.output_guard(crossterm::cursor::Show.to_string());
            let guard = CleanupGuard::new(move || {
                drop(guard);
            });
            _ = write!(output, "{}", crossterm::cursor::Hide);
            self.guard = Some(guard);
        }

        self.buffer.clear();
        write!(self.buffer, "\r").unwrap();
        let control_chars = self.buffer.len();
        write!(self.buffer, "{: >3.0}% ", 100.0 * progress.overall).unwrap();
        if let Some(total) = progress.bytes_downloaded {
            let (scaled, prefix) = binary_prefix(total as f32);
            write!(self.buffer, "{scaled: >5.1} {prefix}B ").unwrap();
        }
        if let Some(estimate) = rate {
            let (scaled, prefix) = binary_prefix(estimate);
            write!(self.buffer, "at {scaled: >5.1} {prefix}B/s ").unwrap();
        }

        let bar_width = output
            .term_width()
            .map(usize::from)
            .unwrap_or(0)
            .saturating_sub(self.buffer.len() - control_chars + 2);
        self.buffer.push('[');
        draw_progress(progress.overall, &mut self.buffer, bar_width);
        self.buffer.push(']');

        write!(self.buffer, "{}", Clear(ClearType::UntilNewLine)).unwrap();
        write!(output, "{}", self.buffer)?;
        output.flush()?;
        Ok(())
    }
}

fn draw_progress(progress: f32, buffer: &mut String, width: usize) {
    const CHARS: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
    const RESOLUTION: usize = CHARS.len() - 1;
    let ticks = (width as f32 * progress.clamp(0.0, 1.0) * RESOLUTION as f32).round() as usize;
    let whole = ticks / RESOLUTION;
    for _ in 0..whole {
        buffer.push(CHARS[CHARS.len() - 1]);
    }
    if whole < width {
        let fraction = ticks % RESOLUTION;
        buffer.push(CHARS[fraction]);
    }
    for _ in (whole + 1)..width {
        buffer.push(CHARS[0]);
    }
}

const UPDATE_HZ: u32 = 30;
const INITIAL_DELAY: Duration = Duration::from_millis(250);

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

pub fn snapshot_progress(ui: &Ui) -> Option<impl Fn(&RepoPath) + '_> {
    struct State {
        guard: Option<OutputGuard>,
        output: ProgressOutput<std::io::Stderr>,
        next_display_time: Instant,
    }

    let output = ui.progress_output()?;

    // Don't clutter the output during fast operations.
    let next_display_time = Instant::now() + INITIAL_DELAY;
    let state = Mutex::new(State {
        guard: None,
        output,
        next_display_time,
    });

    Some(move |path: &RepoPath| {
        let mut state = state.lock().unwrap();
        let now = Instant::now();
        if now < state.next_display_time {
            // Future work: Display current path after exactly, say, 250ms has elapsed, to
            // better handle large single files
            return;
        }
        state.next_display_time = now + Duration::from_secs(1) / UPDATE_HZ;

        if state.guard.is_none() {
            state.guard = Some(
                state
                    .output
                    .output_guard(format!("\r{}", Clear(ClearType::CurrentLine))),
            );
        }

        let line_width = state.output.term_width().map(usize::from).unwrap_or(80);
        let max_path_width = line_width.saturating_sub(13); // Account for "Snapshotting "
        let fs_path = path.to_fs_path_unchecked(Path::new(""));
        let (display_path, _) =
            text_util::elide_start(fs_path.to_str().unwrap(), "...", max_path_width);

        _ = write!(
            state.output,
            "\r{}Snapshotting {display_path}",
            Clear(ClearType::CurrentLine),
        );
        _ = state.output.flush();
    })
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;

    #[test]
    fn test_bar() {
        let mut buf = String::new();
        draw_progress(0.0, &mut buf, 10);
        assert_eq!(buf, "          ");
        buf.clear();
        draw_progress(1.0, &mut buf, 10);
        assert_eq!(buf, "██████████");
        buf.clear();
        draw_progress(0.5, &mut buf, 10);
        assert_eq!(buf, "█████     ");
        buf.clear();
        draw_progress(0.54, &mut buf, 10);
        assert_eq!(buf, "█████▍    ");
        buf.clear();
    }

    #[test]
    fn test_update() {
        let start = Instant::now();
        let mut progress = Progress::new(start);
        let mut current_time = start;
        let mut update = |duration, overall| -> String {
            current_time += duration;
            let mut buf = vec![];
            let mut output = ProgressOutput::for_test(&mut buf, 25);
            progress
                .update(
                    current_time,
                    &jj_lib::git::Progress {
                        bytes_downloaded: None,
                        overall,
                    },
                    &mut output,
                )
                .unwrap();
            String::from_utf8(buf).unwrap()
        };
        // First output is after the initial delay
        assert_snapshot!(update(INITIAL_DELAY - Duration::from_millis(1), 0.1), @"");
        assert_snapshot!(update(Duration::from_millis(1), 0.10), @"[?25l\r 10% [█▊                ][K");
        // No updates for the next 30 milliseconds
        assert_snapshot!(update(Duration::from_millis(10), 0.11), @"");
        assert_snapshot!(update(Duration::from_millis(10), 0.12), @"");
        assert_snapshot!(update(Duration::from_millis(10), 0.13), @"");
        // We get an update now that we go over the threshold
        assert_snapshot!(update(Duration::from_millis(100), 0.30), @" 30% [█████▍            ][K");
        // Even though we went over by quite a bit, the new threshold is relative to the
        // previous output, so we don't get an update here
        assert_snapshot!(update(Duration::from_millis(30), 0.40), @"");
    }
}
