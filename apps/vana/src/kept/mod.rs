//! What VANA keeps on the FILES volume beside the ride journals: the rider's
//! bests, each saved ride's facts and the rides the rider hid. Each owns its
//! record, its codec and the state that reads it into memory; `app` turns
//! them into pages.
pub mod bests;
pub mod deleted;
pub mod facts;
