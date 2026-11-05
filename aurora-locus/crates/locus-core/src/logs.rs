//! The node's recent log lines, kept in memory for the console's Logs
//! page: the newest `cap` lines, oldest dropped first.

use std::collections::VecDeque;
use std::sync::Mutex;

pub struct LogRing {
    lines: Mutex<VecDeque<String>>,
    cap: usize,
}

impl LogRing {
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            lines: Mutex::new(VecDeque::with_capacity(cap)),
            cap,
        }
    }

    pub fn push(&self, line: String) {
        let mut lines = self
            .lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if lines.len() == self.cap {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// The newest `n` lines, oldest first.
    #[must_use]
    pub fn recent(&self, n: usize) -> Vec<String> {
        let lines = self
            .lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lines
            .iter()
            .skip(lines.len().saturating_sub(n))
            .cloned()
            .collect()
    }
}

impl Default for LogRing {
    fn default() -> Self {
        Self::new(1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest() {
        let r = LogRing::new(3);
        for i in 0..5 {
            r.push(i.to_string());
        }
        assert_eq!(r.recent(10), ["2", "3", "4"]);
        assert_eq!(r.recent(2), ["3", "4"]);
    }
}
