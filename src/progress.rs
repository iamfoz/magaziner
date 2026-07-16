//! Console progress reporting with three verbosity levels.
//!
//! All reporting methods take `&self` so a single `Progress` can be shared across the
//! worker threads that fetch articles concurrently. [`ProgressBar`] renders a smooth,
//! in-place Unicode bar (block-eighths resolution) and is likewise thread-safe.

use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, PartialEq)]
pub enum Verbosity {
    Quiet,
    Normal,
    Verbose,
}

pub struct Progress {
    verbosity: Verbosity,
}

impl Progress {
    pub fn new(verbosity: Verbosity) -> Self {
        Self { verbosity }
    }

    pub fn verbosity(&self) -> Verbosity {
        self.verbosity
    }

    fn quiet(&self) -> bool {
        matches!(self.verbosity, Verbosity::Quiet)
    }

    /// A top-level pipeline step, e.g. "Fetching issue…".
    pub fn step(&self, message: &str) {
        if !self.quiet() {
            println!("» {}", message);
        }
    }

    /// A plain informational line.
    pub fn info(&self, message: &str) {
        if !self.quiet() {
            println!("  {}", message);
        }
    }

    /// A warning. Printed to stderr unless quiet.
    pub fn warn(&self, message: &str) {
        if !self.quiet() {
            eprintln!("  warning: {}", message);
        }
    }

    /// A completed-file success line.
    pub fn done(&self, output: impl std::fmt::Display) {
        if !self.quiet() {
            println!("✓ Saved {}", output);
        }
    }

    /// Verbose-only diagnostic output.
    pub fn verbose(&self, message: &str) {
        if matches!(self.verbosity, Verbosity::Verbose) {
            println!("    {}", message);
        }
    }
}

/// A thread-safe, in-place progress bar for a phase with a known item count.
///
/// In `Normal` mode it renders a smooth Unicode bar on a single line, updated with a
/// carriage return. In `Verbose` mode it prints one line per completed item (so the
/// bar doesn't fight the detailed logs). In `Quiet` mode it prints nothing.
pub struct ProgressBar<'a> {
    progress: &'a Progress,
    total: usize,
    done: AtomicUsize,
    render_lock: Mutex<()>,
}

impl<'a> ProgressBar<'a> {
    pub fn new(progress: &'a Progress, total: usize) -> Self {
        Self {
            progress,
            total,
            done: AtomicUsize::new(0),
            render_lock: Mutex::new(()),
        }
    }

    /// Mark one item complete and redraw. `label` describes the just-finished item.
    pub fn inc(&self, label: &str) {
        let done = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        match self.progress.verbosity() {
            Verbosity::Quiet => {}
            Verbosity::Verbose => println!("    [{}/{}] {}", done, self.total, label),
            Verbosity::Normal => {
                let _guard = self.render_lock.lock().unwrap();
                let bar = render_bar(done, self.total, 32);
                // \x1b[K clears any leftover text from a previous, longer line.
                print!("\r  {} {}/{}\x1b[K", bar, done, self.total);
                let _ = std::io::stdout().flush();
            }
        }
    }

    /// Finish the bar, moving off the in-place line.
    pub fn finish(&self) {
        if self.progress.verbosity() == Verbosity::Normal {
            println!();
        }
    }
}

/// Render a smooth bar of `width` cells at block-eighths resolution, bracketed like the
/// reference gist: `⎹████▍     ⎸  57%`.
fn render_bar(current: usize, total: usize, width: usize) -> String {
    const EIGHTHS: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
    let fraction = if total == 0 {
        1.0
    } else {
        (current as f64 / total as f64).clamp(0.0, 1.0)
    };

    let filled = fraction * width as f64;
    let full = filled.floor() as usize;
    let remainder = filled - full as f64;

    let mut bar = String::new();
    bar.push('⎹');
    for _ in 0..full.min(width) {
        bar.push('█');
    }
    let mut cells = full.min(width);
    if cells < width {
        let idx = (remainder * 8.0).round() as usize;
        bar.push(EIGHTHS[idx.min(8)]);
        cells += 1;
    }
    for _ in cells..width {
        bar.push(' ');
    }
    bar.push('⎸');
    bar.push_str(&format!(" {:>3.0}%", fraction * 100.0));
    bar
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_bar_empty() {
        let bar = render_bar(0, 10, 20);
        assert!(bar.contains("0%"), "got: {}", bar);
        assert!(bar.starts_with('⎹'));
    }

    #[test]
    fn test_render_bar_full() {
        let bar = render_bar(10, 10, 20);
        assert!(bar.contains("100%"), "got: {}", bar);
        assert!(bar.contains('█'));
    }

    #[test]
    fn test_render_bar_half() {
        let bar = render_bar(5, 10, 20);
        assert!(bar.contains("50%"), "got: {}", bar);
    }

    #[test]
    fn test_render_bar_zero_total() {
        // Degenerate case must not divide by zero.
        let bar = render_bar(0, 0, 10);
        assert!(bar.contains("100%"), "got: {}", bar);
    }
}
