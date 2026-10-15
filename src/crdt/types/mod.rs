pub mod commutative;
pub mod non_commutative;

pub use commutative::{Scalar, Set, Type};
pub use non_commutative::NonComm;

pub mod set {
    pub use super::commutative::set::Set;
}

pub mod non_comm {
    pub use super::non_commutative::NonComm;
}

#[cfg(test)]
pub mod tests;
