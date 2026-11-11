use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, E_FAIL, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CancelWaitableTimer, CreateWaitableTimerExW, INFINITE,
    SetWaitableTimer, TIMER_ALL_ACCESS, WaitForSingleObject,
};
use windows::core::{Error, PCWSTR, Result};

/// `SetTimer` only ticks every ~15.6 ms, too coarse for 60 fps. Ticks are one-shot and re-armed
/// by [`Self::frame_done`], so they never pile up behind a slow frame.
pub struct FrameTimer {
    timer: HANDLE,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    interval: Duration,
    next: Option<Instant>,
}

impl FrameTimer {
    pub fn new(on_tick: impl Fn() + Send + 'static) -> Result<Self> {
        let timer = unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
            // High resolution needs Windows 10 1803 or later.
            .or_else(|_| CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0))?
        };
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let raw_timer = timer.0 as isize;
        let thread = std::thread::Builder::new()
            .name("nijimi frame timer".into())
            .spawn(move || {
                let timer = HANDLE(raw_timer as *mut std::ffi::c_void);
                while unsafe { WaitForSingleObject(timer, INFINITE) } == WAIT_OBJECT_0
                    && !thread_stop.load(Ordering::Acquire)
                {
                    on_tick();
                }
            });
        let thread = match thread {
            Ok(thread) => thread,
            Err(error) => {
                unsafe {
                    let _ = CloseHandle(timer);
                }
                return Err(Error::new(E_FAIL, error.to_string()));
            }
        };
        Ok(Self {
            timer,
            stop,
            thread: Some(thread),
            interval: Duration::ZERO,
            next: None,
        })
    }

    pub fn start(&mut self, fps: u32) -> Result<()> {
        self.interval = Duration::from_secs(1) / fps.max(1);
        let next = Instant::now() + self.interval;
        self.next = Some(next);
        self.arm(next)
    }

    pub fn frame_done(&mut self) -> Result<()> {
        let Some(previous) = self.next else {
            return Ok(());
        };
        let now = Instant::now();
        let mut next = previous + self.interval;
        if next <= now {
            next = now + self.interval;
        }
        self.next = Some(next);
        self.arm(next)
    }

    pub fn stop(&mut self) {
        self.next = None;
        unsafe {
            let _ = CancelWaitableTimer(self.timer);
        }
    }

    fn arm(&self, at: Instant) -> Result<()> {
        // Negative due times are relative, in 100 ns units.
        let ticks = (at.saturating_duration_since(Instant::now()).as_nanos() / 100).max(1);
        let due = -(ticks as i64);
        unsafe { SetWaitableTimer(self.timer, &due, 0, None, None, false) }
    }
}

impl Drop for FrameTimer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.arm(Instant::now());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        unsafe {
            let _ = CloseHandle(self.timer);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn ticks_at_the_requested_rate_and_stops() {
        let (sender, receiver) = mpsc::channel();
        let mut timer = FrameTimer::new(move || {
            let _ = sender.send(Instant::now());
        })
        .unwrap();
        timer.start(60).unwrap();
        let mut ticks = Vec::new();
        for _ in 0..10 {
            ticks.push(receiver.recv_timeout(Duration::from_secs(1)).unwrap());
            timer.frame_done().unwrap();
        }
        let average = (ticks[9] - ticks[0]) / 9;
        assert!(
            average > Duration::from_millis(12) && average < Duration::from_millis(25),
            "60 fps should tick about every 16.7 ms, got {average:?}"
        );

        timer.stop();
        while receiver.try_recv().is_ok() {}
        assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
    }
}
