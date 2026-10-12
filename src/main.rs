mod app;
mod artwork;
mod config;
mod lyrics;
mod mpris;
mod visualizer;

fn main() -> cosmic::iced::Result {
    cosmic::applet::run::<app::AppModel>(())
}
