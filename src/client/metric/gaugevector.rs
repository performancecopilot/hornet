use super::*;

/// A gauge vector for multiple floating point values with helper methods
/// for incrementing and decrementing their value
///
/// Internally uses an `InstanceMetric<f64>` with `Semantics::Instant` and
/// `Count::One` scale, and `1` count dimension
pub struct GaugeVector {
    im: InstanceMetric<f64>,
    indom: Indom,
    init_val: f64
}

impl GaugeVector {
    /// Creates a new gauge vector with given initial value and instances
    pub fn new(name: &str, init_val: f64, instances: &[&str],
        shorthelp_text: &str, longhelp_text: &str) -> Result<Self, String> {
        
        let indom_helptext = format!("Instance domain for GaugeVector '{}'", name);
        let indom = Indom::new(instances, &indom_helptext, &indom_helptext)?;
        
        let im = InstanceMetric::new(
            &indom,
            name,
            init_val,
            Semantics::Counter,
            Unit::new().count(Count::One, 1)?,
            shorthelp_text,
            longhelp_text
        )?;

        Ok(GaugeVector {
            im: im,
            indom: indom,
            init_val: init_val
        })
    }

    /// Returns the current gauge of the instance
    pub fn val(&self, instance: &str) -> Option<f64> {
        self.im.val(instance)
    }

    /// Sets the gauge of the instance
    pub fn set(&mut self, instance: &str, val: f64) -> Option<io::Result<()>> {
        self.im.set_val(instance, val)
    }

    /// Increments the gauge of the instance by the given value
    ///
    /// The wrapping `Option` is `None` if the instance wasn't found
    pub fn inc(&mut self, instance: &str, increment: f64) -> Option<io::Result<()>> {
        self.im.val(instance).and_then(|val|
            self.im.set_val(instance, val + increment)
        )
    }

    /// Decrements the gauge of the instance by the given value
    ///
    /// The wrapping `Option` is `None` if the instance wasn't found
    pub fn dec(&mut self, instance: &str, decrement: f64) -> Option<io::Result<()>> {
        self.inc(instance, -decrement)
    }

    /// Increments the gauge of all instances by the given value
    pub fn inc_all(&mut self, increment: f64) -> io::Result<()> {
        for instance in self.indom.instances_iter() {
            let val = self.im.val(instance).unwrap();
            self.im.set_val(instance, val + increment).unwrap()?;
        }
        Ok(())
    }

    /// Decrements the gauge of all instances by the given value
    pub fn dec_all(&mut self, decrement: f64) -> io::Result<()> {
        self.inc_all(-decrement)
    }

    /// Resets the gauge of the instance to the initial value that
    /// was passed when creating the vector
    ///
    /// The wrapping `Option` is `None` if the instance wasn't found
    pub fn reset(&mut self, instance: &str) -> Option<io::Result<()>> {
        self.im.set_val(instance, self.init_val)
    }

    /// Resets the gauge of all instances to the initial value that
    /// was passed when creating the vector
    pub fn reset_all(&mut self) -> io::Result<()> {
        for instance in self.indom.instances_iter() {
            self.im.set_val(instance, self.init_val).unwrap()?;
        }
        Ok(())
    }

    /// Internally created instance domain
    pub fn indom(&self) -> &Indom { &self.indom }
}

impl AsRef<InstanceMetric<f64>> for GaugeVector {
    fn as_ref(&self) -> &InstanceMetric<f64> {
        &self.im
    }
}

impl AsMut<InstanceMetric<f64>> for GaugeVector {
    fn as_mut(&mut self) -> &mut InstanceMetric<f64> {
        &mut self.im
    }
}

#[test]
pub fn test() {
    use super::super::Client;

    let mut gv = GaugeVector::new(
        "gauge_vector",
        1.5,
        &["a", "b", "c"],
        "", "").unwrap();

    assert_eq!(gv.val("a").unwrap(), 1.5);   
    assert_eq!(gv.val("b").unwrap(), 1.5);
    assert_eq!(gv.val("c").unwrap(), 1.5);

    Client::new("count_vector_test").unwrap()
        .begin_all(1, 3, 1, 0).unwrap()
        .register_instance_metric(&mut gv).unwrap()
        .export().unwrap();
    
    gv.set("a", 2.5).unwrap().unwrap();
    assert_eq!(gv.val("a").unwrap(), 2.5);

    gv.inc("b", 1.5).unwrap().unwrap();
    assert_eq!(gv.val("b").unwrap(), 3.0);

    gv.dec("c", 1.5).unwrap().unwrap();
    assert_eq!(gv.val("c").unwrap(), 0.0);

    gv.inc_all(2.0).unwrap();
    assert_eq!(gv.val("a").unwrap(), 4.5);   
    assert_eq!(gv.val("b").unwrap(), 5.0);
    assert_eq!(gv.val("c").unwrap(), 2.0);

    gv.dec_all(0.5).unwrap();
    assert_eq!(gv.val("a").unwrap(), 4.0);   
    assert_eq!(gv.val("b").unwrap(), 4.5);
    assert_eq!(gv.val("c").unwrap(), 1.5);

    gv.reset("b").unwrap().unwrap();
    assert_eq!(gv.val("b").unwrap(), 1.5);

    gv.reset_all().unwrap();
    assert_eq!(gv.val("a").unwrap(), 1.5);   
    assert_eq!(gv.val("b").unwrap(), 1.5);
    assert_eq!(gv.val("c").unwrap(), 1.5);
}
