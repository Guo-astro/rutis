// A service method parameter whose type other languages cannot send.
pub struct Shop;
pub struct Item {
    pub n: u32,
}

#[rutis::service]
impl Shop {
    pub fn buy(&self, item: Item) -> u32 {
        item.n
    }
}

fn main() {}
