#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Marker {
    None,
    Missing,
    Deleted,
    Default,
    Stripped,
}
