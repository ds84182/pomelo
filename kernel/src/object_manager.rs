// Manages kernel objects for each process

use std::collections::BTreeMap;

use xorshift::{Xorshift128, Rng, SeedableRng};

pub struct ObjectTable<H: ObjectHandle, T> {
    objects: BTreeMap<H, T>,
    xorshift: Xorshift128,
}

pub trait ObjectHandle: Sized + Ord + Clone {
    fn from(value: u32) -> Option<Self>;
    fn raw(&self) -> u32;
}

impl<H: ObjectHandle, T> ObjectTable<H, T> {
    pub fn new() -> Self {
        ObjectTable {
            objects: BTreeMap::new(),
            xorshift: SeedableRng::from_seed(&([1234, 5678])[..])
        }
    }

    pub fn insert(&mut self, value: T) -> H {
        loop {
            match H::from(self.xorshift.next_u32()) {
                Some(handle) => {
                    if !self.objects.contains_key(&handle) {
                        self.objects.insert(handle.clone(), value);
                        break handle;
                    }
                },
                None => ()
            }
        }
    }

    pub fn lookup(&self, handle: &H) -> Option<&T> {
        self.objects.get(handle)
    }

    pub fn remove(&mut self, handle: &H) -> Option<T> {
        self.objects.remove(handle)
    }
}
