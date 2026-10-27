// A service method whose error type is neither serde nor `Display`.
pub struct Shop;
pub struct Oops;

#[rutis::service]
impl Shop {
    pub fn buy(&self) -> Result<u32, Oops> {
        Err(Oops)
    }
}

fn main() {}
