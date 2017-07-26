use super::*;
use time;
use time::Tm;

/// A timer metric for tracking elapsed time
///
/// Internally uses a `Metric<i64>` with `Semantics::Instant` and `1` time dimension
pub struct Timer {
    metric: Metric<i64>,
    time_scale: Time,
    start_time: Option<Tm>
}

impl Timer {
    /// Creates a new timer metric with given time scale
    pub fn new(name: &str, time_scale: Time,
        shorthelp_text: &str, longhelp_text: &str) -> Result<Self, String> {

        let metric = Metric::new(
            name,
            0,
            Semantics::Instant,
            Unit::new().time(time_scale, 1)?,
            shorthelp_text,
            longhelp_text
        )?;

        Ok(Timer {
            metric: metric,
            time_scale: time_scale,
            start_time: None
        })
    }

    /// Starts the timer. Returns an error if the timer is
    /// already started.
    pub fn start(&mut self) -> Result<(), String> {
        if self.start_time.is_some() {
            return Err("Timer already started!".to_owned());
        }
        self.start_time = Some(time::now());
        Ok(())
    }

    /// Stops the timer, updates the internal metric, and
    /// returns the time elapsed since the last `start`. If
    /// the timer was stopped too early or too late such that
    /// the internal nanosecond, microsecond or millisecond value
    /// over/under-flows, then elapsed time isn't updated.
    /// 
    /// Returns `0` if the timer wasn't started before.
    pub fn stop(&mut self) -> io::Result<i64> {
        let result = match self.start_time {
            Some(start_time) => {
                let duration = time::now() - start_time;

                let elapsed = match self.time_scale {
                    Time::NSec => duration.num_nanoseconds().unwrap_or(0),
                    Time::USec => duration.num_microseconds().unwrap_or(0),
                    Time::MSec => duration.num_microseconds().unwrap_or(0),
                    Time::Sec => duration.num_seconds(),
                    Time::Min => duration.num_minutes(),
                    Time::Hour => duration.num_hours()
                };

                let val = self.metric.val();
                self.metric.set_val(val + elapsed)?;
                Ok(elapsed)
            },
            None => Ok(0)
        };

        self.start_time = None;
        result
    }

    /// Returns the cumulative time elapsed between every
    /// `start` and `stop` pair.
    pub fn elapsed(&mut self) -> i64 {
        self.metric.val()
    }
}

impl AsRef<Metric<i64>> for Timer {
    fn as_ref(&self) -> &Metric<i64> {
        &self.metric
    }
}

impl AsMut<Metric<i64>> for Timer {
    fn as_mut(&mut self) -> &mut Metric<i64> {
        &mut self.metric
    }
}

#[test]
pub fn test() {
    use super::super::Client;
    use std::thread;
    use std::time::Duration;

    let mut timer = Timer::new("timer", Time::MSec, "", "").unwrap();
    assert_eq!(timer.elapsed(), 0);

    Client::new("timer_test").unwrap()
        .begin_metrics(1).unwrap()
        .register_metric(&mut timer).unwrap()
        .export().unwrap();

    assert_eq!(timer.stop().unwrap(), 0);
    
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
