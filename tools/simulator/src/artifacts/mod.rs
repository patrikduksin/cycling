//! Encoders that turn simulator recordings into reviewer-friendly artifacts.

mod canvas;
mod chart;
mod video;

pub use canvas::{Canvas, rgb565_to_rgb8};
pub use chart::{Series, chart_png, chart_svg};
pub use video::{Still, gif, mp4};
