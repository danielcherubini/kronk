use ratatui::prelude::*;

pub struct StatsWidget;

impl StatsWidget {
    pub fn render(f: &mut Frame, area: Rect) {
        let bg = Background(Color::Rgb(40, 44, 52));
        let chunk = f.render_widget(&StatsWidget {}, area, bg);
    }
}
