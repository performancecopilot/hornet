use super::*;

/// A gauge metric for floating point values with helper methods
/// for incrementing and decrementing it's value
///
/// Internally uses a `Metric<f64>` with `Semantics::Instant`,
/// `Count::One` scale, and `1` count dimension
pub struct Gauge {
    metric: Metric<f64>,
    init_val: f64
}

impl Gauge {
    /// Creates a new gauge metric with given initial value
    pub fn new(name: &str, init_val: f64, shorthelp_text: &str, longhelp_text: &str) -> Result<Self, String> {
        let metric = Metric::new(
            name,
            init_val,
            Semantics::Instant,
            Unit::new().count(Count::One, 1)?,
            shorthelp_text,
            longhelp_text
        )?;

        Ok(Gauge {
            metric: metric,
            init_val: init_val
        })
    }

    /// Returns the current value of the gauge
    pub fn val(&self) -> f64 {
        self.metric.val()
    }

    /// Sets the value of the gauge
    pub fn set(&mut self, val: f64) -> io::Result<()> {
        self.metric.set_val(val)
    }

    /// Increments the gauge by the given value
    pub fn inc(&mut self, increment: f64) -> io::Result<()> {
        let val = self.metric.val();
        self.metric.set_val(val + increment)
    }

    /// Decrements the gauge by the given value
    pub fn dec(&mut self, decrement: f64) -> io::Result<()> {
        let val = self.metric.val();
        self.metric.set_val(val - decrement)
    }

    /// Resets the gauge to the initial value that was passed when
    /// creating it
    pub fn reset(&mut self) -> io::Result<()> {
        self.metric.set_val(self.init_val)
    }
}

impl AsRef<Metric<f64>> for Gauge {
    fn as_ref(&self) -> &Metric<f64> {
        &self.metric
    }
}

impl AsMut<Metric<f64>> for Gauge {
    fn as_mut(&mut self) -> &mut Metric<f64> {
        &mut self.metric
    }
}

#[test]
pub fn test() {
    use super::super::Client;

    let mut gauge = Gauge::new("gauge", 1.5, "", "").unwrap();
    assert_eq!(gauge.val(), 1.5);

    Client::new("gauge_test").unwrap()
        .begin_metrics(1).unwrap()
        .register_metric(&mut gauge).unwrap()
        .export().unwrap();
    
    gauge.set(3.0).unwrap();
    assert_eq!(gauge.val(), 3.0);

    gauge.inc(3.0).unwrap();
    assert_eq!(gauge.val(), 6.0);

    gauge.dec(1.5).unwrap();
    assert_eq!(gauge.val(), 4.5);

    gauge.reset().unwrap();
    assert_eq!(gauge.val(), 1.5);
}
