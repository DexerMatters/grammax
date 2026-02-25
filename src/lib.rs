pub mod grammar;
pub mod parsec;

pub mod runtime;
pub mod utils;

pub mod semantic;

#[cfg(any(feature = "webui", feature = "vsclsp"))]
pub mod interface;

#[cfg(test)]
mod tests {

    #[test]
    fn it_works() {}
}
