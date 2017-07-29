use super::*;

/// A count vector for multiple strictly increasing integer values, in possibly
/// varying increments
///
/// Internally uses an `InstanceMetric<u64>` with `Semantics::Counter` and
/// `Count::One` scale, and `1` count dimension
pub struct CountVector {
    im: InstanceMetric<u64>,
    indom: Indom,
    init_val: u64
}

impl CountVector {
    /// Creates a new count vector with given initial value and instances
    pub fn new(name: &str, init_val: u64, instances: &[&str],
        shorthelp_text: &str, longhelp_text: &str) -> Result<Self, String> {
        
        let indom_helptext = format!("Instance domain for CounterVector '{}'", name);
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

        Ok(CountVector {
            im: im,
            indom: indom,
            init_val: init_val
        })
    }

    /// Returns the current count of the instance
    pub fn val(&self, instance: &str) -> Option<u64> {
        self.im.val(instance)
    }

    /// Increments the count of the instance by the given value
    ///
    /// The wrapping `Option` is `None` if the instance wasn't found
    pub fn inc(&mut self, instance: &str, increment: u64) -> Option<io::Result<()>> {
        self.im.val(instance).and_then(|val|
            self.im.set_val(instance, val + increment)
        )
    }

    /// Increments the count of the instance by `+1`
    ///
    /// The wrapping `Option` is `None` if the instance wasn't found
    pub fn up(&mut self, instance: &str) -> Option<io::Result<()>> {
        self.inc(instance, 1)
    }

    /// Increments the count of all instances by the given value
    pub fn inc_all(&mut self, increment: u64) -> io::Result<()> {
        for instance in self.indom.instances_iter() {
            let val = self.im.val(instance).unwrap();
            self.im.set_val(instance, val + increment).unwrap()?;
        }
        Ok(())
    }

    /// Increments the count of all instances by `+1`
    pub fn up_all(&mut self) -> io::Result<()> {
        self.inc_all(1)
    }

    /// Resets the count of the instance to the initial value that
    /// was passed when creating the vector
    ///
    /// The wrapping `Option` is `None` if the instance wasn't found
    pub fn reset(&mut self, instance: &str) -> Option<io::Result<()>> {
        self.im.set_val(instance, self.init_val)
    }

    /// Resets the count of all instances to the initial value that
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

impl AsRef<InstanceMetric<u64>> for CountVector {
    fn as_ref(&self) -> &InstanceMetric<u64> {
        &self.im
    }
}

impl AsMut<InstanceMetric<u64>> for CountVector {
    fn as_mut(&mut self) -> &mut InstanceMetric<u64> {
        &mut self.im
    }
}

#[test]
pub fn test() {
    use super::super::Client;

    let mut cv = CountVector::new(
        "count_vector",
        1,
        &["a", "b", "c"],
        "", "").unwrap();

    assert_eq!(cv.val("a").unwrap(), 1);   
    assert_eq!(cv.val("b").unwrap(), 1);
    assert_eq!(cv.val("c").unwrap(), 1);

    Client::new("count_vector_test").unwrap()
        .begin_all(1, 3, 1, 0).unwrap()
        .register_instance_metric(&mut cv).unwrap()
        .export().unwrap();
    
    cv.up("b").unwrap().unwrap();
    assert_eq!(cv.val("b").unwrap(), 2);

    cv.inc("c", 3).unwrap().unwrap();
    assert_eq!(cv.val("c").unwrap(), 4);

    cv.inc_all(2).unwrap();
    assert_eq!(cv.val("a").unwrap(), 3);   
    assert_eq!(cv.val("b").unwrap(), 4);
    assert_eq!(cv.val("c").unwrap(), 6);

    cv.up_all().unwrap();
    assert_eq!(cv.val("a").unwrap(), 4);   
    assert_eq!(cv.val("b").unwrap(), 5);
    assert_eq!(cv.val("c").unwrap(), 7);

    cv.reset("b").unwrap().unwrap();
    assert_eq!(cv.val("b").unwrap(), 1);

    cv.reset_all().unwrap();
    assert_eq!(cv.val("a").unwrap(), 1);   
    assert_eq!(cv.val("b").unwrap(), 1);
    assert_eq!(cv.val("c").unwrap(), 1);
}
