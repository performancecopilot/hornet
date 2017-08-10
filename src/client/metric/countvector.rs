use std::collections::HashMap;
use super::*;

/// A count vector for multiple strictly increasing integer values, in possibly
/// varying increments
///
/// Internally uses an `InstanceMetric<u64>` with `Semantics::Counter` and
/// `Count::One` scale, and `1` count dimension
pub struct CountVector {
    im: InstanceMetric<u64>,
    indom: Indom,
    init_vals: HashMap<String, u64>
}

impl CountVector {
    /// Creates a new count vector with given instances and a single initial value
    pub fn new(name: &str, init_val: u64, instances: &[&str],
        shorthelp_text: &str, longhelp_text: &str) -> Result<Self, String> {
        
        let mut instances_and_initvals = Vec::new();
        for instance in instances {
            instances_and_initvals.push((*instance, init_val));
        }

        Self::new_with_initvals(
            name,
            &instances_and_initvals,
            shorthelp_text,
            longhelp_text
        )
    }

    /// Creates a new count vector with given pairs of an instance and it's initial value
    pub fn new_with_initvals(name: &str, instances_and_initvals: &[(&str, u64)],
        shorthelp_text: &str, longhelp_text: &str) -> Result<Self, String> {
        
        let mut instances = Vec::new();
        for &(instance, _) in instances_and_initvals.iter() {
            instances.push(instance);
        }

        let indom_helptext = format!("Instance domain for CounterVector '{}'", name);
        let indom = Indom::new(
            &instances,
            &indom_helptext, &indom_helptext
        )?;
        
        let mut im = InstanceMetric::new(
            &indom,
            name,
            0,
            Semantics::Counter,
            Unit::new().count(Count::One, 1)?,
            shorthelp_text,
            longhelp_text
        )?;

        let mut init_vals = HashMap::new();
        for &(instance, init_val) in instances_and_initvals.iter() {
            init_vals.insert(instance.to_owned(), init_val);
            im.set_val(instance, init_val).unwrap().unwrap();
        }

        Ok(CountVector {
            im: im,
            indom: indom,
            init_vals: init_vals
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

    /// Resets the count of the instance to it's initial value that
    /// was passed when creating the vector
    ///
    /// The wrapping `Option` is `None` if the instance wasn't found
    pub fn reset(&mut self, instance: &str) -> Option<io::Result<()>> {
        self.im.set_val(instance, *self.init_vals.get(instance).unwrap())
    }

    /// Resets the count of all instances to it's initial value that
    /// was passed when creating the vector
    pub fn reset_all(&mut self) -> io::Result<()> {
        for (instance, init_val) in self.init_vals.iter() {
            self.im.set_val(instance, *init_val).unwrap()?;
        }
        Ok(())
    }

    /// Internally created instance domain
    pub fn indom(&self) -> &Indom { &self.indom }
}

impl MMVWriter for CountVector {
    private_impl!{}

    fn write(&mut self, ws: &mut MMVWriterState, c: &mut Cursor<&mut [u8]>, mmv_ver: Version) -> io::Result<()> {
        self.im.write(ws, c, mmv_ver)
    }

    fn register(&self, ws: &mut MMVWriterState, mmv_ver: Version) {
        self.im.register(ws, mmv_ver)
    }

    fn has_mmv2_string(&self) -> bool {
        self.im.has_mmv2_string()
    }
}

#[test]
pub fn test() {
    use super::super::Client;

    let mut cv = CountVector::new(
        "count_vector",
        1,
        &["a", "b", "c"],
        "", ""
    ).unwrap();

    assert_eq!(cv.val("a").unwrap(), 1);   
    assert_eq!(cv.val("b").unwrap(), 1);
    assert_eq!(cv.val("c").unwrap(), 1);

    Client::new("count_vector_test").unwrap()
        .export(&mut [&mut cv]).unwrap();
    
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

#[test]
pub fn test_multiple_initvals() {
    use super::super::Client;

    let mut cv = CountVector::new_with_initvals(
        "count_vector_mutiple_initvals",
        &[("a", 1), ("b", 2), ("c", 3)],
        "", ""
    ).unwrap();

    assert_eq!(cv.val("a").unwrap(), 1);   
    assert_eq!(cv.val("b").unwrap(), 2);
    assert_eq!(cv.val("c").unwrap(), 3);

    Client::new("count_vector_test").unwrap()
        .export(&mut [&mut cv]).unwrap();
    
    cv.up_all().unwrap();
    assert_eq!(cv.val("a").unwrap(), 2);   
    assert_eq!(cv.val("b").unwrap(), 3);
    assert_eq!(cv.val("c").unwrap(), 4);

    cv.reset("b").unwrap().unwrap();
    assert_eq!(cv.val("b").unwrap(), 2);

    cv.reset_all().unwrap();
    assert_eq!(cv.val("a").unwrap(), 1);   
    assert_eq!(cv.val("b").unwrap(), 2);
    assert_eq!(cv.val("c").unwrap(), 3);
}
