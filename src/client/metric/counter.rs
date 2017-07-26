use super::*;

/// A counter metric for strictly increasing integer values, in
/// possibly varying increments.
///
/// Internally uses a `Metric<u64>` with `Semantics::Counter` and
/// `Count::One` scale, and `1` count dimension
pub struct Counter {
    metric: Metric<u64>,
    init_val: u64
}

impl Counter {
    /// Creates a new counter metric with given initial value
    pub fn new(name: &str, init_val: u64, shorthelp_text: &str, longhelp_text: &str) -> Result<Self, String> {
        let metric = Metric::new(
            name,
            init_val,
            Semantics::Counter,
            Unit::new().count(Count::One, 1)?,
            shorthelp_text,
            longhelp_text
        )?;

        Ok(Counter {
            metric: metric,
            init_val: init_val
        })
    }

    /// Returns the current value of the counter
    pub fn val(&self) -> u64 {
        self.metric.val()
    }

    /// Increments the counter by the given value
    pub fn inc(&mut self, increment: u64) -> io::Result<()> {
        let val = self.metric.val();
        self.metric.set_val(val + increment)
    }

    /// Increments the counter by `+1`
    pub fn up(&mut self) -> io::Result<()> {
        self.inc(1)
    }

    /// Resets the counter to the initial value that was passed when
    /// creating it
    pub fn reset(&mut self) -> io::Result<()> {
        self.metric.set_val(self.init_val)
    }
}

impl AsRef<Metric<u64>> for Counter {
    fn as_ref(&self) -> &Metric<u64> {
        &self.metric
    }
}

impl AsMut<Metric<u64>> for Counter {
    fn as_mut(&mut self) -> &mut Metric<u64> {
        &mut self.metric
    }
}

#[test]
pub fn test() {
    use super::super::Client;

    let mut counter = Counter::new("counter", 1, "", "").unwrap();
    assert_eq!(counter.val(), 1);

    Client::new("counter_test").unwrap()
        .begin_metrics(1).unwrap()
        .register_metric(&mut counter).unwrap()
        .export().unwrap();
    
    counter.up().unwrap();
    assert_eq!(counter.val(), 2);

    counter.inc(3).unwrap();
    assert_eq!(counter.val(), 5);

    counter.reset().unwrap();
    assert_eq!(counter.val(), 1);
}
