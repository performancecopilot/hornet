use super::*;
use hdrsample;
use hdrsample::Histogram as HdrHist;

/// A histogram metric that records data and reports statistics
///
/// Internally backed by a [HDR Histogram](https://github.com/jonhoo/hdrsample),
/// much of API and documentation being borrowed from it.
///
/// Exports the `max`, `min`, `mean` and `stdev` statistics to an MMV
/// by using an `InstanceMetric<f64>` with `Semantics::Instant`.
pub struct Histogram {
    im: InstanceMetric<f64>,
    indom: Indom,
    histogram: HdrHist<u64>
}

const MAX_INST: &str = "max";
const MIN_INST: &str = "min";
const MEAN_INST: &str = "mean";
const STDEV_INST: &str = "stdev";

const HIST_INSTANCES: &[&str] = &[MAX_INST, MIN_INST, MEAN_INST, STDEV_INST];

/// Error encountered while creating a histogram
#[derive(Debug)]
pub enum CreationError {
    /// Instance error
    Instance(String),
    /// HDR Histogram creation error
    HdrHist(hdrsample::CreationError)
}

impl From<String> for CreationError {
    fn from(err: String) -> CreationError {
        CreationError::Instance(err)
    }
}

impl From<hdrsample::CreationError> for CreationError {
    fn from(err: hdrsample::CreationError) -> CreationError {
        CreationError::HdrHist(err)
    }
}

/// Error encountered while a histogram records values
#[derive(Debug)]
pub enum RecordError {
    /// IO error
    Io(io::Error),
    /// HDR histogram record error
    HdrHist(hdrsample::RecordError)
}

impl From<io::Error> for RecordError {
    fn from(err: io::Error) -> RecordError {
        RecordError::Io(err)
    }
}

impl From<hdrsample::RecordError> for RecordError {
    fn from(err: hdrsample::RecordError) -> RecordError {
        RecordError::HdrHist(err)
    }
}

impl Histogram {
    /// Creates a new histogram metric
    ///
    /// Internally creates a corresponding HDR histogram with auto-resizing disabled
    pub fn new(name: &str, low: u64, high: u64, sigfig: u8, unit: Unit,
        shorthelp_text: &str, longhelp_text: &str) -> Result<Self, CreationError> {
    
        let indom_helptext = format!("Instance domain for Histogram '{}'", name);
        let indom = Indom::new(HIST_INSTANCES, &indom_helptext, &indom_helptext).unwrap();
        
        let im = InstanceMetric::new(
            &indom,
            name,
            0.0,
            Semantics::Instant,
            unit,
            shorthelp_text,
            longhelp_text
        )?;

        let mut histogram = HdrHist::<u64>::new_with_bounds(low, high, sigfig)?;
        histogram.auto(false);

        Ok(Histogram {
            im: im,
            indom: indom,
            histogram: histogram
        })
    }

    fn update_instances(&mut self) -> io::Result<()> {
        self.im.set_val(MIN_INST, self.histogram.min() as f64).unwrap()?;
        self.im.set_val(MAX_INST, self.histogram.max() as f64).unwrap()?;
        self.im.set_val(MEAN_INST, self.histogram.mean()).unwrap()?;
        self.im.set_val(STDEV_INST, self.histogram.stdev()).unwrap()
    }

    /// Records a value
    pub fn record(&mut self, val: u64) -> Result<(), RecordError> {
        self.histogram.record(val)?;
        self.update_instances()?;
        Ok(())
    }

    /// Records multiple samples of a single value
    pub fn record_n(&mut self, val: u64, n: u64) -> Result<(), RecordError> {
        self.histogram.record_n(val, n)?;
        self.update_instances()?;
        Ok(())
    }

    /// Resets the contents and statistics of the histogram
    pub fn reset(&mut self) -> io::Result<()> {
        self.histogram.reset();
        self.update_instances()
    }

    /// Lowest discernible value
    pub fn low(&self) -> u64 { self.histogram.low() }
    /// Highest trackable value
    pub fn high(&self) -> u64 { self.histogram.high() }
    /// Significant value digits
    pub fn significant_figures(&self) -> u8 { self.histogram.sigfig() }
    /// Total number of samples recorded so far
    pub fn count(&self) -> u64 { self.histogram.count() }
    /// Number of distinct values that can currently be represented
    pub fn len(&self) -> usize { self.histogram.len() }

    /// Lowest recorded value
    ///
    /// If no values are yet recorded `0` is returned
    pub fn min(&self) -> u64 { self.histogram.min() }

    /// Highest recorded value
    ///
    /// If no values are yet recorded, an undefined value is returned
    pub fn max(&self) -> u64 { self.histogram.max() }
    
    /// Mean of recorded values
    pub fn mean(&self) -> f64 { self.histogram.mean() }
    
    /// Standard deviation of recorded values
    pub fn stdev(&self) -> f64 { self.histogram.stdev() }

    /// Returns corresponding value at percentile
    pub fn value_at_percentile(&self, percentile: f64) -> u64 {
        self.histogram.value_at_percentile(percentile)
    }

    /// Control whether or not the histogram can auto-resize and auto-adjust
    /// it's highest trackable value as high-valued samples are recorded
    pub fn set_autoresize(&mut self, enable: bool) {
        self.histogram.auto(enable);
    }

    /// Internally created instance domain
    pub fn indom(&self) -> &Indom { &self.indom }

    /// Internally created HDR histogram
    pub fn hdr_histogram(&self) -> &HdrHist<u64> { &self.histogram }
}

impl AsRef<InstanceMetric<f64>> for Histogram {
    fn as_ref(&self) -> &InstanceMetric<f64> {
        &self.im
    }
}

impl AsMut<InstanceMetric<f64>> for Histogram {
    fn as_mut(&mut self) -> &mut InstanceMetric<f64> {
        &mut self.im
    }
}

#[test]
pub fn test() {
    use super::super::Client;
    use rand::{thread_rng, Rng};
    use rand::distributions::{IndependentSample, Range};

    let low = 1;
    let high = 60 * 60 * 1000;
    let sigfig = 2;

    let mut hist = Histogram::new(
        "histogram",
        low, high, sigfig,
        Unit::new(),
        "", ""
    ).unwrap();

    Client::new("histogram_test").unwrap()
        .begin_all(1, 4, 1, 0).unwrap()
        .register_instance_metric(&mut hist).unwrap()
        .export().unwrap();
    
    let val_range = Range::new(low, high);
    let mut rng = thread_rng();

    let n = thread_rng().gen::<u64>() % 100;
    for _ in 0..n { 
        hist.record(val_range.ind_sample(&mut rng)).unwrap();
    }
    hist.record_n(val_range.ind_sample(&mut rng), n).unwrap();

    assert_eq!(
        hist.im.val(MIN_INST).unwrap(),
        hist.histogram.min() as f64
    );

    assert_eq!(
        hist.im.val(MAX_INST).unwrap(),
        hist.histogram.max() as f64
    );

    assert_eq!(
        hist.im.val(MEAN_INST).unwrap(),
        hist.histogram.mean()
    );

    assert_eq!(
        hist.im.val(STDEV_INST).unwrap(),
        hist.histogram.stdev()
    );
}
