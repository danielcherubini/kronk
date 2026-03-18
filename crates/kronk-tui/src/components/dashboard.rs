use ratatui::prelude::*;

pub struct DashboardWidget;

impl DashboardWidget {
    pub fn render(f: &mut Frame, area: Rect) {
        let chunk = f.render_widget(&DashboardWidget {}, area);
    }
}
