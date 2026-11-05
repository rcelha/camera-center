use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// Single-value mailbox: `put` replaces any unread value, so consumers only
/// ever see the latest item and slow consumers drop frames instead of
/// building up latency.
pub struct Latest<T> {
    value: Mutex<Option<T>>,
    ready: Condvar,
}

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self {
            value: Mutex::new(None),
            ready: Condvar::new(),
        }
    }
}

impl<T> Latest<T> {
    pub fn put(&self, v: T) {
        *self.value.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
        self.ready.notify_one();
    }

    /// Waits up to `timeout` for a value.
    pub fn take(&self, timeout: Duration) -> Option<T> {
        let guard = self.value.lock().unwrap_or_else(|e| e.into_inner());
        let (mut guard, _) = self
            .ready
            .wait_timeout_while(guard, timeout, |v| v.is_none())
            .unwrap_or_else(|e| e.into_inner());
        guard.take()
    }

    pub fn wake(&self) {
        self.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_latest() {
        let s = Latest::default();
        s.put(1);
        s.put(2);
        assert_eq!(s.take(Duration::ZERO), Some(2));
        assert_eq!(s.take(Duration::from_millis(5)), None);
    }
}
