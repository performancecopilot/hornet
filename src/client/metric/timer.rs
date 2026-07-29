use super::*;
use std::time::Instant;

/// A timer metric for tracking elapsed time
///
/// Internally uses a `Metric<i64>` with `Semantics::Instant` and `1` time dimension
pub struct Timer {
    metric: Metric<i64>,
    time_scale: Time,
    start_time: Option<Instant>,
}

/// Error encountered while starting or stopping a timer
#[derive(Debug)]
pub enum Error {
    /// IO error
    Io(io::Error),
    /// Timer was already started
    TimerAlreadyStarted,
    /// Timer wasn't previously started
    TimerNotStarted,
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::Io(err)
    }
}

impl Timer {
    /// Creates a new timer metric with given time scale
    pub fn new(
        name: &str,
        time_scale: Time,
        shorthelp_text: &str,
        longhelp_text: &str,
    ) -> Result<Self, String> {
        let metric = Metric::new(
            name,
            0,
            Semantics::Instant,
            Unit::new().time(time_scale, 1)?,
            shorthelp_text,
            longhelp_text,
        )?;

        Ok(Timer {
            metric: metric,
            time_scale: time_scale,
            start_time: None,
        })
    }

    /// Starts the timer. Returns an error if the timer is
    /// already started.
    pub fn start(&mut self) -> Result<(), Error> {
        if self.start_time.is_some() {
            return Err(Error::TimerAlreadyStarted);
        }
        self.start_time = Some(Instant::now());
        Ok(())
    }

    /// Stops the timer, updates the internal metric, and
    /// returns the time elapsed since the last `start`. If
    /// the timer was stopped too early or too late such that
    /// the internal nanosecond, microsecond or millisecond value
    /// over/under-flows, then elapsed time isn't updated.
    pub fn stop(&mut self) -> Result<i64, Error> {
        match self.start_time {
            Some(start_time) => {
                let duration = start_time.elapsed();

                let elapsed = match self.time_scale {
                    Time::NSec => duration.as_nanos() as i64,
                    Time::USec => duration.as_micros() as i64,
                    Time::MSec => duration.as_micros() as i64,
                    Time::Sec => duration.as_secs() as i64,
                    Time::Min => (duration.as_secs() / 60) as i64,
                    Time::Hour => (duration.as_secs() / 3600) as i64,
                };

                let val = *self.metric.val();
                self.metric.set_val(val + elapsed)?;

                // we need to record the time elapsed even if stop()
                // was called before a single unit of time_scale passed
                if elapsed != 0 {
                    self.start_time = None;
                }

                Ok(elapsed)
            }
            None => Err(Error::TimerNotStarted),
        }
    }

    /// Returns the cumulative time elapsed between every
    /// `start` and `stop` pair.
    pub fn elapsed(&mut self) -> i64 {
        *self.metric.val()
    }
}

impl MMVWriter for Timer {
    private_impl! {}

    fn write(
        &mut self,
        ws: &mut MMVWriterState,
        c: &mut Cursor<&mut [u8]>,
        mmv_ver: Version,
    ) -> io::Result<()> {
        self.metric.write(ws, c, mmv_ver)
    }

    fn register(&self, ws: &mut MMVWriterState, mmv_ver: Version) {
        self.metric.register(ws, mmv_ver)
    }

    fn has_mmv2_string(&self) -> bool {
        self.metric.has_mmv2_string()
    }
}

#[test]
pub fn test() {
    use super::super::Client;
    use std::thread;
    use std::time::Duration;

    let mut timer = Timer::new("timer", Time::MSec, "", "").unwrap();
    assert_eq!(timer.elapsed(), 0);

    Client::new("timer_test")
        .unwrap()
        .export(&mut [&mut timer])
        .unwrap();

    assert!(timer.stop().is_err());

    let sleep_time = 2; // seconds

    timer.start().unwrap();
    assert!(timer.start().is_err());
    thread::sleep(Duration::from_secs(sleep_time));
    let elapsed1 = timer.stop().unwrap();
    assert_eq!(timer.elapsed(), elapsed1);

    timer.start().unwrap();
    thread::sleep(Duration::from_secs(sleep_time));
    let elapsed2 = timer.stop().unwrap();
    assert_eq!(timer.elapsed(), elapsed1 + elapsed2);
}
