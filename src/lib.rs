pub mod grammar;
pub mod parsec;
// pub mod parser;

pub mod runtime;
pub mod utils;

// Re-exports for convenience
pub use grammar::Grammar;

#[cfg(test)]
mod tests {

    #[test]
    fn it_works() {}
}
