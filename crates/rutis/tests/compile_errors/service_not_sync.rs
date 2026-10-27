// A service type other plugins cannot share between threads.
pub struct Shop(std::cell::Cell<u32>);

#[rutis::service]
impl Shop {
    pub fn count(&self) -> u32 {
        self.0.get()
    }
}

fn main() {}
